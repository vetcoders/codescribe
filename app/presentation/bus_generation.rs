//! Daily journal generations owned by transcript_bus_maintenance.
//! The manifest links storage bytes; it never closes a take or admits delivery.
use base64::Engine;
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

const SCHEMA: &str = "codescribe.bus-generations.v1";
const CHUNK_SCHEMA: &str = "codescribe.bus-chunk.v1";
const CHUNK_BYTES: usize = 32 * 1024;
const MANIFEST_LIMIT: u64 = 4 << 20;
pub const DECODE_LIMIT: usize = 256 << 20;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unverified bus storage linkage")
}
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}
pub fn manifest_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.generations.json", path.display()))
}
fn lock_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.generation.lock", path.display()))
}

/// All cooperating appenders take this same OS lease before descriptor refresh.
/// The leased section performs metadata and append work, never compression.
/// Remount recovery also verifies recorded archive digests once before rebinding.
/// Concurrent appenders retain their original publication order.
pub struct Lease(File);
impl Lease {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(lock_path(path))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub id: String,
    pub path: PathBuf,
    pub start: u64,
    pub length: u64,
    pub dev: u64,
    pub ino: u64,
    pub day: Option<String>,
    pub compressed: bool,
    /// Verified logical bytes after archive processing, including a successful
    /// attempt that the filesystem retained without compression.
    pub sha256: Option<String>,
    pub superseded: Option<RetiredCopy>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetiredCopy {
    path: PathBuf,
    dev: u64,
    ino: u64,
    length: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub closed: Segment,
    pub next: Segment,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub root: PathBuf,
    pub stream_id: String,
    pub stream_inode: u64,
    pub stream_dev: u64,
    pub stream_birthtime: Option<f64>,
    /// Stable volume identity; unlike st_dev, the macOS volume UUID survives reboot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_uuid: Option<String>,
    pub segments: Vec<Segment>,
    pub active: Segment,
    pub pending: Option<Pending>,
}
fn read_manifest(path: &Path) -> io::Result<Option<Manifest>> {
    let file = match File::open(manifest_path(path)) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if file.metadata()?.len() > MANIFEST_LIMIT {
        return Err(invalid());
    }
    let manifest: Manifest =
        serde_json::from_reader(file.take(MANIFEST_LIMIT)).map_err(|_| invalid())?;
    if manifest.schema != SCHEMA || manifest.root != absolute(path)? {
        return Err(invalid());
    }
    Ok(Some(manifest))
}
pub fn write_manifest(path: &Path, manifest: &Manifest) -> io::Result<()> {
    let target = manifest_path(path);
    let stage = target.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&stage)?;
    serde_json::to_writer(&mut file, manifest).map_err(io::Error::other)?;
    file.sync_all()?;
    fs::rename(&stage, &target)?;
    File::open(target.parent().ok_or_else(invalid)?)?.sync_all()
}
fn same_file(path: &Path, segment: &Segment) -> io::Result<bool> {
    let meta = fs::metadata(path)?;
    Ok(meta.dev() == segment.dev && meta.ino() == segment.ino)
}

#[cfg(target_os = "macos")]
fn volume_uuid(file: &File) -> Option<String> {
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    #[repr(C)]
    struct VolumeAttributes {
        length: u32,
        uuid: [u8; 16],
    }
    let mut result = VolumeAttributes {
        length: 0,
        uuid: [0; 16],
    };
    // fgetattrlist binds the UUID to the opened descriptor, not a second path lookup.
    let status = unsafe {
        libc::fgetattrlist(
            file.as_raw_fd(),
            std::ptr::from_mut(&mut attributes).cast(),
            std::ptr::from_mut(&mut result).cast(),
            std::mem::size_of::<VolumeAttributes>(),
            0,
        )
    };
    (status == 0
        && result.length as usize == std::mem::size_of::<VolumeAttributes>()
        && result.uuid != [0; 16])
        .then(|| Uuid::from_bytes(result.uuid).to_string())
}
#[cfg(not(target_os = "macos"))]
fn volume_uuid(_file: &File) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn boot_time() -> Option<f64> {
    let mut time = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut length = std::mem::size_of::<libc::timeval>();
    let status = unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            std::ptr::from_mut(&mut time).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && length == std::mem::size_of::<libc::timeval>() && time.tv_sec > 0)
        .then_some(time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0)
}
#[cfg(not(target_os = "macos"))]
fn boot_time() -> Option<f64> {
    None
}

fn birthtime(meta: &fs::Metadata) -> Option<f64> {
    meta.created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs_f64())
}

/// Recover only a proven physical-device renumbering. The logical stream clock
/// and identity remain unchanged, so existing cursors and ACKs keep their meaning.
/// This is also callable during startup, before constructing persistent publishers.
pub fn recover_device_number(path: &Path) -> io::Result<bool> {
    let _lease = Lease::acquire(path)?;
    recover_device_number_leased(path)
}

fn recover_device_number_leased(path: &Path) -> io::Result<bool> {
    let Some(manifest) = read_manifest(path)? else {
        return Ok(false);
    };
    if let Ok(mut effective) = resolved(path, manifest.clone()) {
        if effective.volume_uuid.is_none() {
            effective.volume_uuid = volume_uuid(&File::open(path)?);
            if effective.volume_uuid.is_some() {
                write_manifest(path, &effective)?;
            }
        }
        return Ok(false);
    }
    let recovered = rebind_device_number(path, manifest, boot_time())?;
    write_manifest(path, &recovered)?;
    tracing::info!("bus storage device number recovered; logical stream identity retained");
    Ok(true)
}

fn rebind_device_number(
    path: &Path,
    mut manifest: Manifest,
    boot: Option<f64>,
) -> io::Result<Manifest> {
    // A simultaneously interrupted rollover needs its own pinned transaction
    // recovery. Device drift must never guess which side of a pending swap won.
    if manifest.pending.is_some() || manifest.active.path != absolute(path)? {
        return Err(invalid());
    }
    let old_dev = manifest.active.dev;
    let active_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let current_dev = active_file.metadata()?.dev();
    let current_volume = volume_uuid(&active_file);
    if old_dev == current_dev {
        return Err(invalid());
    }
    let known_volume = manifest.volume_uuid.is_some();
    if known_volume {
        if manifest.volume_uuid != current_volume {
            return Err(invalid());
        }
    } else {
        // Older receipts lack a volume UUID. Admission requires independent
        // reboot and original-file birthtime evidence, not just matching inodes.
        let boot = boot.ok_or_else(invalid)?;
        let receipt = fs::symlink_metadata(manifest_path(path))?;
        if !receipt.is_file()
            || receipt.mtime() as f64 + receipt.mtime_nsec() as f64 / 1e9 >= boot
            || current_volume.is_none()
            || manifest.stream_dev != old_dev
        {
            return Err(invalid());
        }
        let original = manifest
            .segments
            .iter()
            .chain(std::iter::once(&manifest.active))
            .find(|segment| segment.ino == manifest.stream_inode)
            .ok_or_else(invalid)?;
        let meta = fs::symlink_metadata(&original.path)?;
        if manifest.stream_birthtime.is_none() || manifest.stream_birthtime != birthtime(&meta) {
            return Err(invalid());
        }
    }
    let mut checked = Vec::new();
    let events = absolute(path)?.parent().ok_or_else(invalid)?.join("events");
    for segment in manifest
        .segments
        .iter_mut()
        .chain(std::iter::once(&mut manifest.active))
    {
        let active = segment.path == absolute(path)?;
        if !active
            && (!segment.path.starts_with(&events)
                || segment
                    .path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir)))
        {
            return Err(invalid());
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&segment.path)?;
        let before = source_identity(&file.metadata()?);
        if segment.dev != old_dev
            || before.dev != current_dev
            || before.ino != segment.ino
            || (!active && before.length != segment.length)
            || (active && before.length < segment.length)
            || current_volume != volume_uuid(&file)
            || segment.superseded.is_some()
        {
            return Err(invalid());
        }
        if let Some(digest) = &segment.sha256 {
            if digest_file(&segment.path)? != (segment.length, digest.clone()) {
                return Err(invalid());
            }
        } else if !known_volume {
            let boot = boot.ok_or_else(invalid)?;
            let unchanged_content = if active {
                // Opening the active writer can chmod the existing inode before
                // append preparation fails. That updates ctime without changing
                // content. Its pre-boot birth and content mtime still establish
                // continuity, including growth since the receipt's checkpoint.
                birthtime(&file.metadata()?).is_some_and(|created| created < boot)
                    && before.modified as f64 + before.modified_ns as f64 / 1e9 < boot
            } else {
                before.changed as f64 + before.changed_ns as f64 / 1e9 < boot
            };
            if !unchanged_content {
                // No historical digest exists. A post-boot content write cannot
                // be distinguished from replacement of the original prefix.
                return Err(invalid());
            }
        }
        if source_identity(&file.metadata()?) != before {
            return Err(invalid());
        }
        checked.push((segment.path.clone(), before));
        segment.dev = current_dev;
    }
    manifest.volume_uuid = current_volume;
    // Reuse the ordinary ordered-chain/path/length validation before publication.
    let effective = resolved(path, manifest)?;
    for (path, before) in checked {
        if source_identity(&fs::symlink_metadata(path)?) != before {
            return Err(invalid());
        }
    }
    Ok(effective)
}
fn resolved(path: &Path, mut manifest: Manifest) -> io::Result<Manifest> {
    if let Some(pending) = manifest.pending.clone() {
        if same_file(path, &pending.next)? {
            manifest.segments.push(pending.closed);
            manifest.active = pending.next;
            manifest.active.path = absolute(path)?;
            manifest.pending = None;
        } else if !same_file(path, &manifest.active)? {
            return Err(invalid());
        }
    }
    let mut start = 0;
    let events = manifest.root.parent().ok_or_else(invalid)?.join("events");
    for segment in &manifest.segments {
        if segment.start != start
            || !segment.path.starts_with(&events)
            || segment
                .path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(invalid());
        }
        let meta = fs::metadata(&segment.path)?;
        if meta.len() != segment.length || meta.dev() != segment.dev || meta.ino() != segment.ino {
            return Err(invalid());
        }
        start = start.checked_add(segment.length).ok_or_else(invalid)?;
    }
    if manifest.active.start != start || !same_file(path, &manifest.active)? {
        return Err(invalid());
    }
    manifest.active.path = absolute(path)?;
    manifest.active.length = fs::metadata(path)?.len();
    Ok(manifest)
}
/// Readers use exactly this ordered chain, including a recoverable pending swap.
pub fn view(path: &Path) -> io::Result<Option<Manifest>> {
    read_manifest(path)?
        .map(|manifest| resolved(path, manifest))
        .transpose()
}
fn new_segment(path: &Path, start: u64, day: Option<String>) -> io::Result<Segment> {
    let meta = fs::metadata(path)?;
    Ok(Segment {
        id: Uuid::new_v4().to_string(),
        path: absolute(path)?,
        start,
        length: meta.len(),
        dev: meta.dev(),
        ino: meta.ino(),
        day,
        compressed: false,
        sha256: None,
        superseded: None,
    })
}
/// Called under the OS lease. Existing undated bytes are pinned without scanning
/// or compressing; mtime is never used to invent their calendar provenance.
pub fn prepare_append(path: &Path, file: &mut File) -> io::Result<Option<String>> {
    let path = absolute(path)?;
    if !path.exists() {
        *file = super::super::transcript_bus::open_bus_append_file(&path)?;
    }
    let today = Local::now().format("%Y_%m%d").to_string();
    let mut manifest = if let Some(original) = read_manifest(&path)? {
        let effective = match resolved(&path, original.clone()) {
            Ok(effective) => effective,
            Err(_) => {
                recover_device_number_leased(&path)?;
                resolved(&path, read_manifest(&path)?.ok_or_else(invalid)?)?
            }
        };
        if original.pending.is_some() && effective.pending.is_none() {
            write_manifest(&path, &effective)?;
        }
        effective
    } else {
        let meta = fs::metadata(&path)?;
        let day = (meta.len() == 0).then(|| today.clone());
        let value = Manifest {
            schema: SCHEMA.into(),
            root: path.clone(),
            stream_id: Uuid::new_v4().to_string(),
            stream_inode: meta.ino(),
            stream_dev: meta.dev(),
            stream_birthtime: birthtime(&meta),
            volume_uuid: volume_uuid(file),
            segments: Vec::new(),
            active: new_segment(&path, 0, day)?,
            pending: None,
        };
        write_manifest(&path, &value)?;
        value
    };
    if !same_descriptor(file, &manifest.active) {
        *file = super::super::transcript_bus::open_bus_append_file(&path)?;
    }
    if manifest.active.day.as_deref() == Some(today.as_str()) {
        return Ok(None);
    }
    // Any trailing fragment remains on the original authority; never append a
    // new generation that would make an incomplete row disappear to guards.
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut byte = [0];
        file.read_exact(&mut byte)?;
        if byte[0] != b'\n' {
            return Err(invalid());
        }
    }
    let mut closed = manifest.active.clone();
    closed.length = file.metadata()?.len();
    let directory = path
        .parent()
        .ok_or_else(invalid)?
        .join("events")
        .join(closed.day.as_deref().unwrap_or("undated"));
    fs::create_dir_all(&directory)?;
    let retired = directory.join(format!("{}.jsonl", closed.id));
    let next_path = directory.join(format!("{}.hot", Uuid::new_v4()));
    let replacement = OpenOptions::new()
        .create_new(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .open(&next_path)?;
    replacement.sync_all()?;
    file.sync_all()?;
    // Reuse a prepared transaction only when its pinned identity still matches.
    if let Some(pending) = manifest.pending.take() {
        if pending.closed.id != closed.id
            || pending.closed.start != closed.start
            || pending.closed.length != closed.length
            || pending.closed.dev != closed.dev
            || pending.closed.ino != closed.ino
            || !same_file(&pending.closed.path, &closed)?
            || fs::metadata(&pending.closed.path)?.len() != closed.length
            || !pending
                .next
                .path
                .starts_with(path.parent().ok_or_else(invalid)?.join("events"))
            || pending
                .next
                .path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
            || !same_file(&pending.next.path, &pending.next)?
            || fs::metadata(&pending.next.path)?.len() != 0
            || pending.next.start
                != closed
                    .start
                    .checked_add(closed.length)
                    .ok_or_else(invalid)?
        {
            return Err(invalid());
        }
        fs::remove_file(&next_path)?;
        let replacement = super::super::transcript_bus::open_bus_append_file(&pending.next.path)?;
        fs::rename(&pending.next.path, &path)?;
        *file = replacement;
        manifest.segments.push(pending.closed.clone());
        manifest.active = pending.next;
        manifest.active.path = path.clone();
        write_manifest(&path, &manifest)?;
        return Ok(pending.closed.day.map(|_| pending.closed.id));
    }
    match fs::hard_link(&path, &retired) {
        Ok(()) => {}
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists
                && same_file(&retired, &closed)?
                && fs::metadata(&retired)?.len() == closed.length => {}
        Err(e) => return Err(e),
    }
    File::open(&directory)?.sync_all()?;
    closed.path = retired;
    let next = new_segment(
        &next_path,
        closed
            .start
            .checked_add(closed.length)
            .ok_or_else(invalid)?,
        Some(today),
    )?;
    manifest.pending = Some(Pending {
        closed: closed.clone(),
        next: next.clone(),
    });
    write_manifest(&path, &manifest)?;
    fs::rename(&next_path, &path)?;
    File::open(path.parent().ok_or_else(invalid)?)?.sync_all()?;
    *file = replacement;
    manifest.segments.push(closed.clone());
    manifest.active = next;
    manifest.active.path = path.clone();
    manifest.pending = None;
    write_manifest(&path, &manifest)?;
    Ok(closed.day.map(|_| closed.id))
}
fn same_descriptor(file: &File, segment: &Segment) -> bool {
    file.metadata()
        .is_ok_and(|meta| meta.dev() == segment.dev && meta.ino() == segment.ino)
}
pub fn append(path: &Path, shared: &Arc<Mutex<File>>, bytes: &[u8]) -> io::Result<()> {
    let mut file = shared.lock().unwrap_or_else(|e| e.into_inner());
    let _lease = Lease::acquire(path)?;
    let closed = prepare_append(path, &mut file)?;
    file.write_all(bytes)?;
    file.flush()?;
    drop(_lease);
    drop(file);
    schedule_archives(path, closed);

    Ok(())
}

/// A durable append receipt uses the logical stream clock, across rollover.
#[derive(Debug, Serialize)]
pub struct BusAppendReceipt {
    pub stream_id: String,
    pub stream_dev: u64,
    pub stream_inode: u64,
    pub offset: u64,
    pub length: u64,
}

/// Reply text must reach the same private journal authority before speech.
/// Keep descriptor refresh, coordinates, write and durability under its lease.
pub fn append_durable(
    path: &Path,
    shared: &Arc<Mutex<File>>,
    bytes: &[u8],
) -> io::Result<BusAppendReceipt> {
    let mut file = shared.lock().unwrap_or_else(|error| error.into_inner());
    let lease = Lease::acquire(path)?;
    let closed = prepare_append(path, &mut file)?;
    let manifest = view(path)?.ok_or_else(invalid)?;
    let offset = manifest
        .active
        .start
        .checked_add(file.metadata()?.len())
        .ok_or_else(invalid)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    let receipt = BusAppendReceipt {
        stream_id: manifest.stream_id,
        stream_dev: manifest.stream_dev,
        stream_inode: manifest.stream_inode,
        offset,
        length: bytes.len() as u64,
    };
    drop(lease);
    drop(file);
    schedule_archives(path, closed);
    Ok(receipt)
}

#[derive(Default)]
struct ArchiveWork {
    pending: HashSet<String>,
    running: bool,
    inspected: bool,
    retry_at: Option<Instant>,
}
static ARCHIVING: OnceLock<Mutex<HashMap<PathBuf, ArchiveWork>>> = OnceLock::new();
/// One metadata-only backlog inspection per process. Empty queues never spawn;
/// failures retain work with a one-minute backoff, rather than repeating ditto
/// for every Word append. New closed generations coalesce under the same owner.
fn schedule_archives(path: &Path, closed: Option<String>) {
    let Ok(path) = absolute(path) else {
        return;
    };
    let workers = ARCHIVING.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let mut state = workers.lock().unwrap_or_else(|e| e.into_inner());
        let work = state.entry(path.clone()).or_default();
        if let Some(id) = closed {
            work.pending.insert(id);
        }
        if !work.inspected
            && let Ok(Some(manifest)) = view(&path)
        {
            work.pending.extend(
                manifest
                    .segments
                    .iter()
                    .filter(|s| {
                        s.day.is_some()
                            && ((!s.compressed && s.sha256.is_none()) || s.superseded.is_some())
                    })
                    .map(|s| s.id.clone()),
            );
            work.inspected = true;
        }
        if work.running
            || work.pending.is_empty()
            || work.retry_at.is_some_and(|at| Instant::now() < at)
        {
            return;
        }
        work.running = true;
    }
    std::thread::spawn(move || {
        loop {
            let ids = {
                let mut state = workers.lock().unwrap_or_else(|e| e.into_inner());
                let work = state.get_mut(&path).expect("archive owner reserved");
                if work.pending.is_empty() {
                    work.running = false;
                    return;
                }
                work.pending.drain().collect::<Vec<_>>()
            };
            let failed = ids
                .into_iter()
                .filter(|id| compress_segment(&path, id).is_err())
                .collect::<Vec<_>>();
            if !failed.is_empty() {
                tracing::warn!(
                    "bus archive compression unavailable; original generations retained"
                );
                let mut state = workers.lock().unwrap_or_else(|e| e.into_inner());
                let work = state.get_mut(&path).expect("archive owner reserved");
                work.pending.extend(failed);
                work.retry_at = Some(Instant::now() + Duration::from_secs(60));
                work.running = false;
                return;
            }
        }
    });
}

/// An immutable view of the linked byte stream. At most one segment descriptor
/// is open per read; seek and cursors use original logical byte coordinates.
pub struct Reader {
    root: PathBuf,
    pub manifest: Option<Manifest>,
    segments: Vec<Segment>,
    position: u64,
    length: u64,
}
impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let manifest = match view(path) {
            Ok(manifest) => manifest,
            Err(_) => {
                recover_device_number(path)?;
                view(path)?
            }
        };
        let segments = if let Some(value) = &manifest {
            let mut segments = value.segments.clone();
            segments.push(value.active.clone());
            segments
        } else {
            vec![new_segment(path, 0, None)?]
        };
        let last = segments.last().ok_or_else(invalid)?;
        let length = last.start.checked_add(last.length).ok_or_else(invalid)?;
        Ok(Self {
            root: absolute(path)?,
            manifest,
            segments,
            position: 0,
            length,
        })
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn identity(&self) -> (u64, u64) {
        self.manifest
            .as_ref()
            .map(|m| (m.stream_dev, m.stream_inode))
            .unwrap_or((self.segments[0].dev, self.segments[0].ino))
    }
}
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        let segment = self
            .segments
            .iter()
            .find(|s| self.position >= s.start && self.position < s.start + s.length)
            .ok_or_else(invalid)?;
        let mut file = match File::open(&segment.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound && self.manifest.is_some() => {
                let latest = view(&self.root)?.ok_or_else(invalid)?;
                let current = latest
                    .segments
                    .iter()
                    .find(|s| {
                        s.id == segment.id && s.start == segment.start && s.length == segment.length
                    })
                    .ok_or_else(invalid)?;
                let file = File::open(&current.path)?;
                if !same_descriptor(&file, current) {
                    return Err(invalid());
                }
                drop(file);
                self.segments = {
                    let mut segments = latest.segments.clone();
                    segments.push(latest.active.clone());
                    segments
                };
                return self.read(out);
            }
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        if meta.dev() != segment.dev || meta.ino() != segment.ino || meta.len() < segment.length {
            return Err(invalid());
        }
        file.seek(SeekFrom::Start(self.position - segment.start))?;
        let width = out
            .len()
            .min((segment.start + segment.length - self.position) as usize);
        let read = file.read(&mut out[..width])?;
        if read == 0 {
            return Err(invalid());
        }
        self.position += read as u64;
        Ok(read)
    }
}
impl Seek for Reader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(p) => self.position as i128 + p as i128,
            SeekFrom::End(p) => self.length as i128 + p as i128,
        };
        if position < 0 || position > u64::MAX as i128 {
            return Err(invalid());
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

fn digest_file(path: &Path) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut block = [0; 64 * 1024];
    loop {
        let n = file.read(&mut block)?;
        if n == 0 {
            break;
        }
        hash.update(&block[..n]);
        bytes += n as u64;
    }
    Ok((bytes, hex::encode(hash.finalize())))
}
#[cfg(target_os = "macos")]
fn compressed(path: &Path) -> io::Result<bool> {
    use std::os::macos::fs::MetadataExt;
    Ok(fs::metadata(path)?.st_flags() & 0x20 != 0)
}
#[cfg(not(target_os = "macos"))]
fn compressed(_path: &Path) -> io::Result<bool> {
    Ok(false)
}
/// This is an off-lock platform operation, never a clone-only success claim.
pub fn compress_segment(root: &Path, id: &str) -> io::Result<()> {
    let original = view(root)?.ok_or_else(invalid)?;
    let segment = original
        .segments
        .iter()
        .find(|s| s.id == id && s.day.is_some())
        .ok_or_else(invalid)?
        .clone();
    if segment.compressed {
        return retire_copy(root, id);
    }
    if segment.sha256.is_some() {
        return Ok(());
    }
    let before = source_identity(&fs::metadata(&segment.path)?);
    if before.dev != segment.dev || before.ino != segment.ino || before.length != segment.length {
        return Err(invalid());
    }
    let Some(stage) = compress_prepared(&segment.path)? else {
        let (length, digest) = digest_file(&segment.path)?;
        if length != segment.length || source_identity(&fs::metadata(&segment.path)?) != before {
            return Err(invalid());
        }
        let _lease = Lease::acquire(root)?;
        let mut manifest = view(root)?.ok_or_else(invalid)?;
        let entry = manifest
            .segments
            .iter_mut()
            .find(|s| s.id == id && s.ino == segment.ino && !s.compressed)
            .ok_or_else(invalid)?;
        if source_identity(&fs::metadata(&entry.path)?) != before {
            return Err(invalid());
        }
        // Keep the original bytes and physical identity. A verified no-benefit
        // result is settled archive work, not a compression failure to retry.
        entry.sha256 = Some(digest);
        return write_manifest(root, &manifest);
    };
    let (_, digest) = digest_file(&stage)?;
    if source_identity(&fs::metadata(&segment.path)?) != before {
        return Err(invalid());
    }
    {
        let _lease = Lease::acquire(root)?;
        let mut manifest = view(root)?.ok_or_else(invalid)?;
        let entry = manifest
            .segments
            .iter_mut()
            .find(|s| s.id == id && s.ino == segment.ino && !s.compressed)
            .ok_or_else(invalid)?;
        let meta = fs::metadata(&stage)?;
        entry.superseded = Some(RetiredCopy {
            path: entry.path.clone(),
            dev: entry.dev,
            ino: entry.ino,
            length: entry.length,
        });
        entry.path = stage;
        entry.dev = meta.dev();
        entry.ino = meta.ino();
        entry.compressed = true;
        entry.sha256 = Some(digest);
        // This durable pointer admits only verified ordinary logical bytes.
        // The original remains until publication succeeds, including crashes.
        write_manifest(root, &manifest)?;
    }
    retire_copy(root, id)
}
fn retire_copy(root: &Path, id: &str) -> io::Result<()> {
    let _lease = Lease::acquire(root)?;
    let mut manifest = view(root)?.ok_or_else(invalid)?;
    let entry = manifest
        .segments
        .iter_mut()
        .find(|s| s.id == id && s.compressed)
        .ok_or_else(invalid)?;
    if let Some(copy) = &entry.superseded {
        let events = absolute(root)?.parent().ok_or_else(invalid)?.join("events");
        if !copy.path.starts_with(events)
            || copy
                .path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(invalid());
        }
        match fs::metadata(&copy.path) {
            Ok(meta)
                if meta.dev() == copy.dev
                    && meta.ino() == copy.ino
                    && meta.len() == copy.length =>
            {
                fs::remove_file(&copy.path)?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(invalid()),
        }
        File::open(copy.path.parent().ok_or_else(invalid)?)?.sync_all()?;
        entry.superseded = None;
        write_manifest(root, &manifest)?;
    }
    Ok(())
}

/// Bounded physical records preserve a shared revision without enlarging any
/// existing consumer's physical line limit. Publication is one append batch.
pub fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() < 512 * 1024 {
        let mut row = bytes;
        row.push(b'\n');
        return Ok(row);
    }
    let id = hex::encode(Sha256::digest(&bytes));
    let document: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let event = serde_json::json!({"schema":document["schema"],"session_id":document["session_id"],"emitted_at":document["emitted_at"],"status":document["status"],"source":document["source"]});
    let parts = bytes.len().div_ceil(CHUNK_BYTES);
    let mut out = Vec::new();
    for (part, payload) in bytes.chunks(CHUNK_BYTES).enumerate() {
        let row = serde_json::json!({"schema":CHUNK_SCHEMA,"id":id,"part":part,"parts":parts,"length":bytes.len(),"event":event,"payload":base64::engine::general_purpose::STANDARD.encode(payload)});
        serde_json::to_writer(&mut out, &row).map_err(io::Error::other)?;
        out.push(b'\n');
    }
    Ok(out)
}
#[derive(Default, Debug)]
pub struct Decoder {
    id: String,
    parts: u64,
    next: u64,
    length: usize,
    pending: Vec<u8>,
    header: Value,
}
impl Decoder {
    pub fn incomplete(&self) -> bool {
        self.parts != 0
    }
    pub fn push(&mut self, value: Value) -> io::Result<Vec<Value>> {
        if value["schema"] != CHUNK_SCHEMA {
            if self.incomplete() {
                return Err(invalid());
            }
            return expand(value);
        }
        let id = value["id"].as_str().ok_or_else(invalid)?;
        let part = value["part"].as_u64().ok_or_else(invalid)?;
        let parts = value["parts"].as_u64().ok_or_else(invalid)?;
        let length = value["length"].as_u64().ok_or_else(invalid)? as usize;
        if length > DECODE_LIMIT || parts == 0 || parts != length.div_ceil(CHUNK_BYTES) as u64 {
            return Err(invalid());
        }
        if part == 0 {
            if self.incomplete() {
                return Err(invalid());
            }
            self.id = id.into();
            self.parts = parts;
            self.next = 0;
            self.length = length;
            self.pending.clear();
            self.header = value["event"].clone();
        }
        if self.id != id
            || self.next != part
            || self.parts != parts
            || self.length != length
            || self.header != value["event"]
        {
            return Err(invalid());
        }
        let payload = base64::engine::general_purpose::STANDARD
            .decode(value["payload"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        if payload.len() > CHUNK_BYTES || self.pending.len() + payload.len() > length {
            return Err(invalid());
        }
        self.pending.extend_from_slice(&payload);
        self.next += 1;
        if self.next != parts {
            return Ok(Vec::new());
        }
        if self.pending.len() != length || hex::encode(Sha256::digest(&self.pending)) != self.id {
            return Err(invalid());
        }
        let row: Value = serde_json::from_slice(&self.pending).map_err(|_| invalid())?;
        let header = self.header.as_object().ok_or_else(invalid)?;
        if header.iter().any(|(key, field)| &row[key] != field) {
            return Err(invalid());
        }
        *self = Self::default();
        expand(row)
    }
}
/// Validate the complete receipt inventory before any consumer acts on a row.
fn expand(value: Value) -> io::Result<Vec<Value>> {
    let _ = rows(value.clone())?;
    Ok(vec![value])
}
pub struct Rows {
    common: Value,
    first: bool,
    occurrences: std::vec::IntoIter<Value>,
}
impl Iterator for Rows {
    type Item = Value;
    fn next(&mut self) -> Option<Value> {
        if self.first {
            self.first = false;
            return Some(self.common.clone());
        }
        let occurrence = self.occurrences.next()?;
        let mut row = self.common.clone();
        for (key, field) in occurrence.as_object()? {
            row.as_object_mut()?.insert(key.clone(), field.clone());
        }
        Some(row)
    }
}
/// Materialize only one original occurrence projection at a time.
pub fn rows(mut value: Value) -> io::Result<Rows> {
    let occurrences = if let Some(encoding) = value.get("persistence_encoding") {
        if encoding != "shared-revision.v1"
            || value["schema"] != "codescribe.transcript-evidence.v1"
        {
            return Err(invalid());
        }
        let fields = value.as_object_mut().ok_or_else(invalid)?;
        fields.remove("persistence_encoding");
        let occurrences = fields.remove("occurrence_rows").ok_or_else(invalid)?;
        let occurrences = occurrences.as_array().ok_or_else(invalid)?.clone();
        let allowed = [
            "sequence",
            "emitted_at",
            "occurrence_session_id",
            "capture_epoch",
            "sample_start",
            "sample_end",
            "document_index",
            "label",
            "acoustic_receipts",
        ];
        for occurrence in &occurrences {
            let fields = occurrence.as_object().ok_or_else(invalid)?;
            if fields.len() != allowed.len()
                || fields.keys().any(|k| !allowed.contains(&k.as_str()))
                || [
                    "sequence",
                    "capture_epoch",
                    "sample_start",
                    "sample_end",
                    "document_index",
                ]
                .iter()
                .any(|k| !fields[*k].is_u64())
                || ["emitted_at", "occurrence_session_id", "label"]
                    .iter()
                    .any(|k| !fields[*k].is_string())
                || !fields["acoustic_receipts"].is_array()
            {
                return Err(invalid());
            }
        }
        occurrences
    } else {
        Vec::new()
    };
    Ok(Rows {
        common: value,
        first: true,
        occurrences: occurrences.into_iter(),
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    dev: u64,
    ino: u64,
    length: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}
fn source_identity(meta: &fs::Metadata) -> SourceIdentity {
    SourceIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
        length: meta.len(),
        modified: meta.mtime(),
        modified_ns: meta.mtime_nsec(),
        changed: meta.ctime(),
        changed_ns: meta.ctime_nsec(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPartition {
    bucket: String,
    path: PathBuf,
    records: u64,
    bytes: u64,
    sha256: String,
    compressed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    schema: String,
    source: PathBuf,
    identity: SourceIdentity,
    sha256: Option<String>,
    output: PathBuf,
    staged: bool,
    compression_requested: bool,
    admission: String,
    partitions: Vec<PreparedPartition>,
}
fn preparation_manifest(output: &Path) -> PathBuf {
    output.join("preparation.json")
}
fn unchanged(source: &Path, file: &File, identity: &SourceIdentity) -> io::Result<()> {
    if source_identity(&file.metadata()?) != *identity
        || source_identity(&fs::metadata(source)?) != *identity
    {
        return Err(io::Error::other(
            "migration source changed; staging remains unadmitted",
        ));
    }
    Ok(())
}
fn partition_day(bytes: &[u8], terminated: bool) -> String {
    if !terminated {
        return "unknown".into();
    }
    let Ok(row) = serde_json::from_slice::<Value>(bytes) else {
        return "unknown".into();
    };
    let known = [
        "codescribe.transcript.v1",
        "codescribe.transcript-evidence.v1",
        "codescribe.transcript-history.v1",
        "codescribe.derived-transcript.v1",
        "codescribe.agent-ack.v1",
        "codescribe.channel-session.v1",
        "codescribe.agent-reply.v1",
    ];
    if !row["schema"].as_str().is_some_and(|s| known.contains(&s)) {
        return "unknown".into();
    }
    row["emitted_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|date| date.with_timezone(&Local).format("%Y_%m%d").to_string())
        .unwrap_or_else(|| "unknown".into())
}
/// Bounded fanout and metadata per calendar bucket, never a per-record manifest.
struct PartitionWriter {
    root: PathBuf,
    partitions: std::collections::BTreeMap<String, PreparedPartition>,
    files: std::collections::VecDeque<(String, File)>,
}
impl PartitionWriter {
    fn write(&mut self, bucket: &str, bytes: &[u8], record_end: bool) -> io::Result<()> {
        if !self.partitions.contains_key(bucket) {
            if self.partitions.len() >= 4096 {
                return Err(io::Error::other(
                    "migration calendar inventory exceeds preparation budget",
                ));
            }
            let directory = self.root.join("events").join(bucket);
            fs::create_dir_all(&directory)?;
            let path = directory.join(format!("{}.jsonl", Uuid::new_v4()));
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            file.sync_all()?;
            self.partitions.insert(
                bucket.into(),
                PreparedPartition {
                    bucket: bucket.into(),
                    path,
                    records: 0,
                    bytes: 0,
                    sha256: String::new(),
                    compressed: false,
                },
            );
        }
        let entry = self.partitions.get_mut(bucket).ok_or_else(invalid)?;
        if let Some(index) = self.files.iter().position(|(name, _)| name == bucket) {
            let file = self.files.remove(index).ok_or_else(invalid)?;
            self.files.push_back(file);
        } else {
            if self.files.len() == 8
                && let Some((_, file)) = self.files.pop_front()
            {
                file.sync_all()?;
            }
            let file = OpenOptions::new().append(true).open(&entry.path)?;
            self.files.push_back((bucket.into(), file));
        }
        self.files
            .back_mut()
            .ok_or_else(invalid)?
            .1
            .write_all(bytes)?;
        entry.bytes = entry
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(invalid)?;
        if record_end {
            entry.records += 1;
        }
        Ok(())
    }
    fn finish(&mut self) -> io::Result<()> {
        for (_, file) in self.files.drain(..) {
            file.sync_all()?;
        }
        for entry in self.partitions.values_mut() {
            let (bytes, hash) = digest_file(&entry.path)?;
            if bytes != entry.bytes {
                return Err(invalid());
            }
            entry.sha256 = hash;
        }
        Ok(())
    }
}
fn compress_prepared(source: &Path) -> io::Result<Option<PathBuf>> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = source;
        Ok(None)
    }
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        let target = source.with_file_name(format!("{}.compressed.jsonl", Uuid::new_v4()));
        let before = source_identity(&fs::metadata(source)?);
        let status = Command::new("/usr/bin/ditto")
            .args(["--noclone", "--rsrc", "--extattr", "--hfsCompression"])
            .arg(source)
            .arg(&target)
            .env_remove("DITTONORSRC")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            let _ = fs::remove_file(&target);
            return Err(io::Error::other(format!(
                "archive compression command failed: {status}"
            )));
        }
        if digest_file(source)? != digest_file(&target)?
            || source_identity(&fs::metadata(source)?) != before
        {
            let _ = fs::remove_file(&target);
            return Err(invalid());
        }
        if !compressed(&target)? {
            fs::remove_file(&target)?;
            return Ok(None);
        }
        File::open(&target)?.sync_all()?;
        Ok(Some(target))
    }
}
/// Explicit source tooling only. Default is metadata planning. Staging neither
/// publishes a generation chain nor deletes/truncates the source. Final cutover
/// is deliberately unavailable until runtime ownership and global order have
/// a separately reviewed admission protocol.
pub fn prepare_migration(
    source: &Path,
    output: &Path,
    stage: bool,
    compression: bool,
    resume: bool,
    apply: bool,
) -> io::Result<Preparation> {
    if apply {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "migration admission is unavailable: writer ownership and original cross-day ordering require deliberate admission",
        ));
    }
    if (compression || resume) && !stage {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "compression and resume require explicit staging",
        ));
    }
    let source = absolute(source)?;
    let output = absolute(output)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&source)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    let identity = source_identity(&file.metadata()?);
    let mut report = Preparation {
        schema: "codescribe.bus-preparation.v1".into(),
        source: source.clone(),
        identity: identity.clone(),
        sha256: None,
        output: output.clone(),
        staged: false,
        compression_requested: compression,
        admission: "unadmitted; original source retains global ordering".into(),
        partitions: Vec::new(),
    };
    if !stage {
        return Ok(report);
    }
    if resume {
        let manifest = File::open(preparation_manifest(&output))?;
        if manifest.metadata()?.len() > MANIFEST_LIMIT {
            return Err(invalid());
        }
        let completed: Preparation =
            serde_json::from_reader(manifest.take(MANIFEST_LIMIT)).map_err(|_| invalid())?;
        if completed.schema != report.schema
            || completed.source != source
            || completed.identity != identity
            || completed.output != output
            || !completed.staged
            || completed.compression_requested != compression
        {
            return Err(invalid());
        }
        unchanged(&source, &file, &identity)?;
        if completed.sha256.as_deref() != Some(digest_file(&source)?.1.as_str()) {
            return Err(invalid());
        }
        for entry in &completed.partitions {
            if !entry.path.starts_with(output.join("events"))
                || entry
                    .path
                    .components()
                    .any(|p| matches!(p, std::path::Component::ParentDir))
            {
                return Err(invalid());
            }
            if digest_file(&entry.path)? != (entry.bytes, entry.sha256.clone())
                || (entry.compressed && !compressed(&entry.path)?)
            {
                return Err(invalid());
            }
        }
        unchanged(&source, &file, &identity)?;
        return Ok(completed);
    }
    // Own a new directory exclusively. Interrupted preparation is preserved;
    // without a complete receipt it cannot be resumed or admitted by accident.
    fs::create_dir(&output)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&output, fs::Permissions::from_mode(0o700))?;
    let mut writer = PartitionWriter {
        root: output.clone(),
        partitions: Default::default(),
        files: Default::default(),
    };
    let mut digest = Sha256::new();
    let mut prefix = Vec::new();
    let mut oversized = false;
    let mut remaining = identity.length;
    let mut block = [0u8; 64 * 1024];
    while remaining > 0 {
        let width = block.len().min(remaining as usize);
        let n = file.read(&mut block[..width])?;
        if n == 0 {
            return Err(invalid());
        }
        remaining -= n as u64;
        digest.update(&block[..n]);
        let mut start = 0;
        while start < n {
            let end = block[start..n]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(n, |p| start + p + 1);
            let terminated = block[end - 1] == b'\n';
            let piece = &block[start..end];
            if !oversized && prefix.len() + piece.len() > 1024 * 1024 {
                writer.write("unknown", &prefix, false)?;
                prefix.clear();
                oversized = true;
            }
            if oversized {
                writer.write("unknown", piece, terminated)?;
            } else {
                prefix.extend_from_slice(piece);
            }
            if terminated {
                if !oversized {
                    let day = partition_day(&prefix, true);
                    writer.write(&day, &prefix, true)?;
                    prefix.clear();
                }
                oversized = false;
            }
            start = end;
        }
    }
    if oversized {
        writer.write("unknown", &[], true)?;
    } else if !prefix.is_empty() {
        writer.write("unknown", &prefix, true)?;
    }
    unchanged(&source, &file, &identity)?;
    writer.finish()?;
    if compression {
        for partition in writer.partitions.values_mut() {
            if let Some(compressed_path) = compress_prepared(&partition.path)? {
                // Original source and ordinary staging copy both remain intact.
                partition.path = compressed_path;
                partition.compressed = true;
            }
            unchanged(&source, &file, &identity)?;
        }
    }
    report.partitions = writer.partitions.into_values().collect();
    if report
        .partitions
        .iter()
        .try_fold(0u64, |total, p| total.checked_add(p.bytes))
        .ok_or_else(invalid)?
        != identity.length
    {
        return Err(invalid());
    }
    report.sha256 = Some(hex::encode(digest.finalize()));
    report.staged = true;
    let temporary = output.join(format!("{}.receipt.tmp", Uuid::new_v4()));
    let mut receipt = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut receipt, &report).map_err(io::Error::other)?;
    receipt.sync_all()?;
    unchanged(&source, &file, &identity)?;
    fs::rename(&temporary, preparation_manifest(&output))?;
    File::open(&output)?.sync_all()?;
    Ok(report)
}

#[cfg(test)]
mod integrator_generation_acceptance {
    use super::*;
    use std::collections::BTreeMap;

    fn pinned_previous_day(path: &Path, contents: &[u8]) -> File {
        let mut file = super::super::super::transcript_bus::open_bus_append_file(path).unwrap();
        prepare_append(path, &mut file).unwrap();
        file.write_all(contents).unwrap();
        file.sync_all().unwrap();
        let mut manifest = view(path).unwrap().unwrap();
        manifest.active.day = Some("2026_0930".into());
        write_manifest(path, &manifest).unwrap();
        file
    }

    #[cfg(target_os = "macos")]
    fn device_drift_fixture(path: &Path) -> (File, Manifest, (u64, u64)) {
        let mut file = pinned_previous_day(path, b"original archive\n");
        prepare_append(path, &mut file).unwrap();
        file.write_all(b"active prefix\n").unwrap();
        file.sync_all().unwrap();
        let mut manifest = view(path).unwrap().unwrap();
        assert!(manifest.volume_uuid.is_some());
        manifest.segments[0].sha256 = Some(digest_file(&manifest.segments[0].path).unwrap().1);
        let identity = (manifest.stream_dev, manifest.stream_inode);
        // Simulate a recorded device number from a previous mount, retaining
        // all real inode, volume, digest and logical stream evidence.
        manifest.active.dev += 1;
        for segment in &mut manifest.segments {
            segment.dev += 1;
        }
        write_manifest(path, &manifest).unwrap();
        (file, manifest, identity)
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn device_drift_reader_recovers_without_changing_bytes_or_cursor_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (_file, stale, identity) = device_drift_fixture(&path);
        let archive = fs::read(&stale.segments[0].path).unwrap();
        let active = fs::read(&path).unwrap();
        let mut reader = Reader::open(&path).unwrap();
        assert_eq!(reader.identity(), identity);
        assert_eq!(reader.manifest.as_ref().unwrap().stream_id, stale.stream_id);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [archive.as_slice(), active.as_slice()].concat());
        assert_eq!(fs::read(&stale.segments[0].path).unwrap(), archive);
        assert_eq!(fs::read(&path).unwrap(), active);
        let receipt = fs::read(manifest_path(&path)).unwrap();
        // A new reader/process observes the durable fix, with no second rewrite.
        assert_eq!(Reader::open(&path).unwrap().identity(), identity);
        assert!(!recover_device_number(&path).unwrap());
        assert_eq!(fs::read(manifest_path(&path)).unwrap(), receipt);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn device_drift_append_refreshes_under_existing_lease_and_preserves_active_growth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (mut file, _, identity) = device_drift_fixture(&path);
        // Manifest length is a checkpoint, not a prohibition on legitimate appends.
        file.write_all(b"appended before recovery\n").unwrap();
        file.sync_all().unwrap();
        let lease = Lease::acquire(&path).unwrap();
        prepare_append(&path, &mut file).unwrap();
        file.write_all(b"appended after recovery\n").unwrap();
        file.sync_all().unwrap();
        drop(lease);
        assert_eq!(Reader::open(&path).unwrap().identity(), identity);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"active prefix\nappended before recovery\nappended after recovery\n"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn device_drift_refuses_replaced_file_and_does_not_rewrite_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (_file, stale, _) = device_drift_fixture(&path);
        let receipt = fs::read(manifest_path(&path)).unwrap();
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, fs::read(&stale.segments[0].path).unwrap()).unwrap();
        fs::rename(replacement, &stale.segments[0].path).unwrap();
        assert!(recover_device_number(&path).is_err());
        assert_eq!(fs::read(manifest_path(&path)).unwrap(), receipt);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn device_drift_refuses_same_inode_same_length_archive_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (_file, stale, _) = device_drift_fixture(&path);
        let receipt = fs::read(manifest_path(&path)).unwrap();
        let original = fs::read(&stale.segments[0].path).unwrap();
        fs::write(&stale.segments[0].path, vec![b'x'; original.len()]).unwrap();
        assert!(recover_device_number(&path).is_err());
        assert_eq!(fs::read(manifest_path(&path)).unwrap(), receipt);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn device_drift_refuses_different_volume_or_partial_transition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (_file, stale, _) = device_drift_fixture(&path);
        let mut foreign = stale.clone();
        foreign.volume_uuid = Some(Uuid::new_v4().to_string());
        write_manifest(&path, &foreign).unwrap();
        assert!(recover_device_number(&path).is_err());
        let mut partial = stale.clone();
        partial.segments[0].dev -= 1;
        write_manifest(&path, &partial).unwrap();
        assert!(recover_device_number(&path).is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pre_uuid_device_drift_requires_reboot_and_original_birthtime_proof() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (_file, mut stale, _) = device_drift_fixture(&path);
        stale.volume_uuid = None;
        stale.stream_dev = stale.active.dev;
        write_manifest(&path, &stale).unwrap();
        assert!(rebind_device_number(&path, stale.clone(), None).is_err());
        let future_boot = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            + 60.0;
        let recovered = rebind_device_number(&path, stale.clone(), Some(future_boot)).unwrap();
        assert!(recovered.volume_uuid.is_some());
        assert_eq!(recovered.stream_dev, stale.stream_dev);
        stale.stream_birthtime = stale.stream_birthtime.map(|time| time - 1.0);
        assert!(rebind_device_number(&path, stale, Some(future_boot)).is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pre_uuid_recovery_allows_postboot_chmod_with_preboot_active_content_growth() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (mut file, mut stale, _) = device_drift_fixture(&path);
        stale.volume_uuid = None;
        stale.stream_dev = stale.active.dev;
        let checkpoint = stale.active.length;
        file.write_all(b"more preboot content\n").unwrap();
        file.sync_all().unwrap();
        write_manifest(&path, &stale).unwrap();
        let receipt = fs::metadata(manifest_path(&path)).unwrap();
        let receipt_time = receipt.mtime() as f64 + receipt.mtime_nsec() as f64 / 1e9;
        let before = fs::read(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let after = file.metadata().unwrap();
        let changed = after.ctime() as f64 + after.ctime_nsec() as f64 / 1e9;
        let boot = (receipt_time + changed) / 2.0;
        assert!(receipt_time < boot && boot < changed);
        let recovered = rebind_device_number(&path, stale.clone(), Some(boot)).unwrap();
        assert!(recovered.active.length > checkpoint);
        assert_eq!(recovered.active.length, before.len() as u64);
        assert_eq!(recovered.stream_id, stale.stream_id);
        assert_eq!(recovered.stream_dev, stale.stream_dev);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn pre_uuid_recovery_still_refuses_postboot_active_content_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let (mut file, mut stale, _) = device_drift_fixture(&path);
        stale.volume_uuid = None;
        stale.stream_dev = stale.active.dev;
        write_manifest(&path, &stale).unwrap();
        let receipt = fs::metadata(manifest_path(&path)).unwrap();
        let receipt_time = receipt.mtime() as f64 + receipt.mtime_nsec() as f64 / 1e9;
        file.write_all(b"postboot content\n").unwrap();
        file.sync_all().unwrap();
        let after = file.metadata().unwrap();
        let modified = after.mtime() as f64 + after.mtime_nsec() as f64 / 1e9;
        let boot = (receipt_time + modified) / 2.0;
        assert!(receipt_time < boot && boot < modified);
        let original_receipt = fs::read(manifest_path(&path)).unwrap();
        let original_content = fs::read(&path).unwrap();
        assert!(rebind_device_number(&path, stale, Some(boot)).is_err());
        assert_eq!(fs::read(manifest_path(&path)).unwrap(), original_receipt);
        assert_eq!(fs::read(&path).unwrap(), original_content);
    }

    #[test]
    fn known_day_rollover_conserves_bytes_and_global_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let before = b"{\"schema\":\"codescribe.transcript.v1\",\"status\":\"session_started\",\"session_id\":\"ongoing\"}\n";
        let after = b"{\"schema\":\"codescribe.transcript.v1\",\"status\":\"session_ended\",\"session_id\":\"ongoing\"}\n";
        let mut file = pinned_previous_day(&path, before);
        let identity = Reader::open(&path).unwrap().identity();
        let id = prepare_append(&path, &mut file).unwrap().unwrap();
        file.write_all(after).unwrap();
        file.flush().unwrap();
        let manifest = view(&path).unwrap().unwrap();
        assert_eq!(manifest.segments.len(), 1);
        assert_eq!(manifest.segments[0].id, id);
        assert_eq!(manifest.segments[0].day.as_deref(), Some("2026_0930"));
        assert_eq!(fs::read(&manifest.segments[0].path).unwrap(), before);
        assert_eq!(fs::read(&path).unwrap(), after);
        let mut reader = Reader::open(&path).unwrap();
        assert_eq!(reader.identity(), identity);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [before.as_slice(), after.as_slice()].concat());
        reader.seek(SeekFrom::Start(before.len() as u64)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, after);
    }

    #[test]
    fn unclassified_original_is_preserved_without_invented_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let old = b"opaque historical payload\n";
        fs::write(&path, old).unwrap();
        let mut file = super::super::super::transcript_bus::open_bus_append_file(&path).unwrap();
        assert!(prepare_append(&path, &mut file).unwrap().is_none());
        let manifest = view(&path).unwrap().unwrap();
        assert_eq!(manifest.segments.len(), 1);
        assert!(manifest.segments[0].day.is_none());
        assert_eq!(fs::read(&manifest.segments[0].path).unwrap(), old);
        assert!(
            manifest.segments[0]
                .path
                .parent()
                .unwrap()
                .ends_with("undated")
        );
        assert!(!manifest.segments[0].compressed);
    }

    #[test]
    fn incomplete_original_refuses_rollover_without_hiding_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let old = b"{\"unfinished\": ";
        fs::write(&path, old).unwrap();
        let mut file = super::super::super::transcript_bus::open_bus_append_file(&path).unwrap();
        assert!(prepare_append(&path, &mut file).is_err());
        assert_eq!(fs::read(&path).unwrap(), old);
        assert!(view(&path).unwrap().unwrap().segments.is_empty());
    }

    #[test]
    fn successful_small_archive_attempt_settles_without_replacing_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let bytes = b"{\"small\":true}\n";
        let mut file = pinned_previous_day(&path, bytes);
        let id = prepare_append(&path, &mut file).unwrap().unwrap();
        let original = view(&path).unwrap().unwrap().segments[0].clone();
        compress_segment(&path, &id).unwrap();
        let settled = view(&path).unwrap().unwrap().segments[0].clone();
        assert!(
            !settled.compressed,
            "tiny archive has no compression benefit"
        );
        assert_eq!(settled.path, original.path);
        assert_eq!(
            (settled.dev, settled.ino, settled.length),
            (original.dev, original.ino, original.length)
        );
        assert_eq!(fs::read(&settled.path).unwrap(), bytes);
        assert_eq!(settled.sha256, Some(digest_file(&settled.path).unwrap().1));
        let manifest_before = fs::read(manifest_path(&path)).unwrap();
        compress_segment(&path, &id).unwrap();
        assert_eq!(fs::read(manifest_path(&path)).unwrap(), manifest_before);
        let mut reader = Reader::open(&path).unwrap();
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read, bytes);
    }

    #[test]
    fn compressed_closed_generation_remains_readable_to_existing_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bus.jsonl");
        let old = ("{\"label\":\"zażółć gęślą jaźń\"}\n")
            .repeat(10000)
            .into_bytes();
        let mut file = pinned_previous_day(&path, &old);
        let id = prepare_append(&path, &mut file).unwrap().unwrap();
        file.write_all(b"later\n").unwrap();
        file.flush().unwrap();
        let mut cursor = Reader::open(&path).unwrap();
        cursor.seek(SeekFrom::Start(31)).unwrap();
        compress_segment(&path, &id).unwrap();
        let manifest = view(&path).unwrap().unwrap();
        assert!(manifest.segments[0].compressed);
        assert!(compressed(&manifest.segments[0].path).unwrap());
        let mut actual = Vec::new();
        cursor.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, [old[31..].to_vec(), b"later\n".to_vec()].concat());
        assert_eq!(fs::read(&manifest.segments[0].path).unwrap(), old);
    }

    #[test]
    fn large_logical_document_has_bounded_lines_and_no_partial_delivery() {
        let value = serde_json::json!({"schema":"codescribe.transcript-evidence.v1","session_id":"chunks","status":"document_revision","source":"raw","emitted_at":"2026-10-01T00:00:00Z","rendered_text":"ż".repeat(350000)});
        let wire = encode(&value).unwrap();
        let mut decoder = Decoder::default();
        let rows: Vec<_> = wire
            .split(|b| *b == b'\n')
            .filter(|r| !r.is_empty())
            .collect();
        assert!(rows.len() > 1);
        for (index, bytes) in rows.iter().enumerate() {
            assert!(bytes.len() < 64 * 1024);
            let out = decoder
                .push(serde_json::from_slice(bytes).unwrap())
                .unwrap();
            if index + 1 == rows.len() {
                assert_eq!(out, vec![value.clone()]);
            } else {
                assert!(out.is_empty());
                assert!(decoder.incomplete());
            }
        }
        assert!(!decoder.incomplete());
        let mut decoder = Decoder::default();
        decoder
            .push(serde_json::from_slice(rows[0]).unwrap())
            .unwrap();
        assert!(
            decoder
                .push(serde_json::from_slice(rows[2]).unwrap())
                .is_err()
        );
    }

    // Self-contained counterparts of the frozen integrator acceptance fixtures.
    // Keep each row on its intended local day without changing process-wide TZ.
    fn fixture_root() -> tempfile::TempDir {
        use chrono::{Local, TimeZone};
        let dir = tempfile::tempdir().unwrap();
        let fixture = r###"{"schema":"codescribe.transcript.v1","session_id":"synthetic-midnight","sequence":1,"status":"session_started","emitted_at":"2026-09-30T23:59:58+02:00"}
{"schema":"codescribe.transcript-evidence.v1","session_id":"synthetic-midnight","sequence":2,"reducer_revision":1,"rendered_text":"Zażółć gęślą jaźń.","emitted_at":"2026-10-01T00:00:02+02:00","lifecycle_terminal":false}
{"schema":"future.private.synthetic.v99","session_id":"synthetic-future","payload":{"keep":"verbatim synthetic payload"},"emitted_at":"2026-09-30T12:00:00+02:00"}
{"schema":"codescribe.transcript.v1","session_id":"synthetic-midnight","sequence":3,"status":"session_ended","emitted_at":"2026-10-01T00:00:03+02:00","text":"Zażółć gęślą jaźń."}
{"schema":"codescribe.transcript-evidence.v1","session_id":"synthetic-old-edit","sequence":4,"reducer_action":"apply_manual_edit","rendered_text":"historia korekty","emitted_at":"2026-09-30T13:00:00+02:00"}
{"schema":"synthetic.undated.v1","payload":"must not acquire invented date"}
"###;
        let mut bytes = Vec::new();
        for line in fixture.lines() {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(timestamp) = row.get("emitted_at").and_then(|v| v.as_str()) {
                let date = chrono::DateTime::parse_from_rfc3339(timestamp)
                    .unwrap()
                    .date_naive();
                let local = Local
                    .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
                    .single()
                    .unwrap();
                row["emitted_at"] = serde_json::Value::String(local.to_rfc3339());
            }
            bytes.extend(serde_json::to_vec(&row).unwrap());
            bytes.push(b'\n');
        }
        fs::write(dir.path().join("mixed-days.jsonl"), &bytes).unwrap();
        let mut oversized = bytes.clone();
        oversized.extend_from_slice(b"{\"schema\":\"codescribe.transcript.v1\",\"emitted_at\":\"2026-09-30T12:00:00Z\",\"payload\":\"");
        oversized.resize(oversized.len() + (3 << 20), b'x');
        oversized.extend_from_slice(b"\"}\n");
        fs::write(dir.path().join("mixed-plus-oversized.jsonl"), oversized).unwrap();
        let mut malformed = bytes.clone();
        malformed.extend_from_slice(b"{\"torn\":");
        fs::write(
            dir.path().join("mixed-plus-malformed-tail.jsonl"),
            malformed,
        )
        .unwrap();
        let mut nonutf8 = bytes.clone();
        nonutf8.extend_from_slice(b"\xff\xfe\n");
        fs::write(dir.path().join("mixed-plus-nonutf8.jsonl"), nonutf8).unwrap();
        fs::write(dir.path().join("empty.jsonl"), []).unwrap();
        dir
    }
    fn source_rows(bytes: &[u8]) -> Vec<&[u8]> {
        bytes.split_inclusive(|b| *b == b'\n').collect()
    }

    #[test]
    fn migration_staging_preserves_every_frozen_byte_and_partition_order() {
        let names = [
            "mixed-days.jsonl",
            "mixed-plus-oversized.jsonl",
            "mixed-plus-malformed-tail.jsonl",
            "mixed-plus-nonutf8.jsonl",
            "empty.jsonl",
        ];
        let fixtures = fixture_root();
        for name in names {
            let source = fixtures.path().join(name);
            let bytes = fs::read(&source).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let output = directory.path().join("stage");
            let report = prepare_migration(&source, &output, true, false, false, false).unwrap();
            assert_eq!(fs::read(&source).unwrap(), bytes);
            assert!(report.staged);
            let rows = source_rows(&bytes);
            // Frozen fixture date/source semantics, independent of partition_day.
            // The future-schema row is preserved as unknown, never discarded.
            let mut expected = BTreeMap::<String, Vec<u8>>::new();
            if !bytes.is_empty() {
                for index in [0usize, 4] {
                    expected
                        .entry("2026_0930".into())
                        .or_default()
                        .extend_from_slice(rows[index]);
                }
                for index in [1usize, 3] {
                    expected
                        .entry("2026_1001".into())
                        .or_default()
                        .extend_from_slice(rows[index]);
                }
                for (index, row) in rows.iter().enumerate() {
                    if ![0usize, 1, 3, 4].contains(&index) {
                        expected
                            .entry("unknown".into())
                            .or_default()
                            .extend_from_slice(row);
                    }
                }
            }
            let actual: BTreeMap<_, _> = report
                .partitions
                .iter()
                .map(|p| (p.bucket.clone(), fs::read(&p.path).unwrap()))
                .collect();
            assert_eq!(actual, expected, "{name}");
            assert_eq!(
                report.partitions.iter().map(|p| p.bytes).sum::<u64>(),
                bytes.len() as u64
            );
            assert!(!manifest_path(&source).exists());
            assert_eq!(
                report.sha256,
                Some(
                    Sha256::digest(&bytes)
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                )
            );
        }
    }

    #[test]
    fn migration_default_and_apply_never_scan_or_mutate_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let bytes = b"unaltered\xff\n";
        fs::write(&source, bytes).unwrap();
        let out = dir.path().join("stage");
        let report = prepare_migration(&source, &out, false, false, false, false).unwrap();
        assert!(!report.staged);
        assert!(report.sha256.is_none());
        assert!(!out.exists());
        assert!(prepare_migration(&source, &out, true, false, false, true).is_err());
        assert!(!out.exists());
        assert_eq!(fs::read(source).unwrap(), bytes);
    }

    #[test]
    fn migration_completed_resume_refuses_source_or_partition_drift() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let fixtures = fixture_root();
        fs::copy(fixtures.path().join("mixed-days.jsonl"), &source).unwrap();
        let out = dir.path().join("stage");
        let prepared = prepare_migration(&source, &out, true, false, false, false).unwrap();
        let resumed = prepare_migration(&source, &out, true, false, true, false).unwrap();
        assert_eq!(resumed.sha256, prepared.sha256);
        let original = fs::read(&source).unwrap();
        fs::write(&prepared.partitions[0].path, b"tampered\n").unwrap();
        assert!(prepare_migration(&source, &out, true, false, true, false).is_err());
        assert_eq!(fs::read(&source).unwrap(), original);
        OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap()
            .write_all(b"extra\n")
            .unwrap();
        assert!(prepare_migration(&source, &out, true, false, true, false).is_err());
    }
}
