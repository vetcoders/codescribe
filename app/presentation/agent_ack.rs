//! Append one `agent_ack` bus row for each acknowledged seal delivery.
//!
//! Markers live at `acknowledgments/<lease>/<delivery_id>.json` and contain
//! the admitted envelope owner. This module does not rewrite bus rows.
//! A delivery is emitted once: the cursor remembers it, and a scan of existing
//! `agent_ack` rows refuses a second append after the cursor is gone.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use std::borrow::Cow;

use super::transcript_bus::shared_bus_file;

const LEASE_SCHEMA: &str = "codescribe.agent-bridge.lease.v1";
const BINDING_SCHEMA: &str = "vc.agent-audience-binding.v1";
const BINDING_FILENAME: &str = "vc.agent-audience-binding.v1.json";
pub(crate) const ACK_SCHEMA: &str = "codescribe.agent-ack.v1";
const CURSOR_SCHEMA: &str = "codescribe.agent-ack-cursor.v1";
pub(crate) const CHANNEL_SESSION_SCHEMA: &str = "codescribe.channel-session.v1";
const CURSOR_FILE: &str = "agent-ack-cursor.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub appended: usize,
    pub skipped_unproven: usize,
}

#[derive(Debug, Default)]
struct Cursor {
    emitted: BTreeSet<String>,
    known_seals: BTreeSet<String>,
    ducked_replies: Vec<String>,
    /// Read position per bus file. The 500 ms scan loop must never re-read
    /// history: without this the pass is O(file) and a long-lived bus pins a
    /// core (observed at 100% CPU on a 20 GB bus).
    bus_marks: BTreeMap<String, BusMark>,
}

/// Where a bus file was last consumed, plus a fingerprint of its first bytes
/// so a rotation to the same byte length is still detected.
#[derive(Debug, Default, Clone, Copy)]
struct BusMark {
    offset: u64,
    head_len: u32,
    head_hash: u64,
    /// Unix inode of the file the mark belongs to. A rename-style rotation
    /// changes it even when length and head bytes happen to match.
    ino: u64,
}

struct ChannelBinding {
    channel: String,
    audience: String,
}

/// Watch the bridge home and append missing seal receipts.
///
/// `fallback_bus` is used when a lease does not name its own bus file.
pub fn scan(bridge_home: &Path, fallback_bus: &Path) -> io::Result<ScanStats> {
    if !bridge_home.is_dir() {
        return Ok(ScanStats::default());
    }
    let mut cursor = load_cursor(bridge_home)?;
    let mut dirty = !cursor_path(bridge_home).is_file();
    let channels = load_channels(bridge_home);
    let leases = load_leases(bridge_home);
    let mut buses = vec![fallback_bus.to_path_buf()];
    for lease in &leases {
        if let Some(bus) = lease.bus.as_ref()
            && !buses.iter().any(|known| known == bus)
        {
            buses.push(bus.clone());
        }
    }
    for bus in &buses {
        let key = bus.to_string_lossy().into_owned();
        let mut mark = cursor.bus_marks.get(&key).copied().unwrap_or_default();
        let before = mark;
        fold_bus_delta(bus, &mut mark, |row| {
            if row.kind.as_deref() == Some("agent_ack")
                && let Some(id) = delivery_id_of(row)
                && cursor.emitted.insert(id.to_string())
            {
                dirty = true;
            }
            // A seal row read once must keep proving its delivery on later
            // passes, which no longer re-read history.
            if let Some(id) = seal_delivery_id(row) {
                let id = id.to_string();
                if !cursor.emitted.contains(&id) && cursor.known_seals.insert(id) {
                    dirty = true;
                }
            }
            if let Some(reply_id) = spoken_reply_id(row) {
                if reply_is_recent(row) {
                    note_spoken_reply(&mut cursor, &mut dirty, &reply_id);
                } else {
                    remember_reply(&mut cursor, &mut dirty, &reply_id);
                }
            }
        })?;
        if mark.offset != before.offset || mark.head_hash != before.head_hash {
            cursor.bus_marks.insert(key, mark);
            dirty = true;
        }
    }
    for lease in &leases {
        for pending in &lease.pending {
            if matches!(pending.kind.as_deref(), Some("seal" | "message"))
                && cursor.known_seals.insert(pending.id.clone())
            {
                dirty = true;
            }
        }
    }

    let mut stats = ScanStats::default();
    let ack_root = bridge_home.join("acknowledgments");
    let lease_dirs = match fs::read_dir(&ack_root) {
        Ok(dirs) => dirs,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if dirty {
                write_cursor(bridge_home, &cursor)?;
            }
            return Ok(stats);
        }
        Err(error) => return Err(error),
    };
    let mut dir_paths: Vec<PathBuf> = lease_dirs
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    dir_paths.sort();
    for dir in dir_paths {
        let Some(lease_id) = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !safe_lease_id(&lease_id) {
            continue;
        }
        let Some(lease) = leases.iter().find(|lease| lease.lease_id == lease_id) else {
            continue;
        };
        let mut markers: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        markers.sort();
        for marker_path in markers {
            let Some(delivery_id) = marker_delivery_id(&marker_path) else {
                continue;
            };
            // A persisted or bus-recovered ack is already final.
            if cursor.emitted.contains(delivery_id) {
                if cursor.known_seals.remove(delivery_id) {
                    dirty = true;
                }
                continue;
            }
            // Bus and pending delivery proof were folded into known_seals
            // before traversal. The marker body cannot authorize an ack alone.
            if !cursor.known_seals.contains(delivery_id) {
                // This counts canonical candidates lacking delivery proof.
                stats.skipped_unproven += 1;
                continue;
            }
            // Proof only permits inspection. A new marker must still pass
            // its file, body, lease, envelope, and recipient checks.
            let Some(marker) = read_marker(&marker_path, lease, fallback_bus)? else {
                continue;
            };
            let delivery_id = marker.delivery_id;
            let Some(binding) = marker.recipient.as_ref().or_else(|| {
                (!marker.has_envelope)
                    .then(|| channels.get(&lease.provider_session_id))
                    .flatten()
            }) else {
                stats.skipped_unproven += 1;
                continue;
            };
            if cursor.emitted.contains(&delivery_id) {
                if cursor.known_seals.remove(&delivery_id) {
                    dirty = true;
                }
                continue;
            }
            let row = json!({
                "schema": ACK_SCHEMA,
                "kind": "agent_ack",
                "channel": binding.channel,
                "delivery_id": delivery_id,
                "agent": binding.audience,
                "provider": lease.provider,
                "provider_session_id": lease.provider_session_id,
                "lease_id": lease.lease_id,
                "emitted_at": utc_now(),
            });
            let bus = lease.bus.as_deref().unwrap_or(fallback_bus);
            append_json_line(bus, &row)?;
            cursor.emitted.insert(delivery_id.clone());
            cursor.known_seals.remove(&delivery_id);
            dirty = true;
            stats.appended += 1;
        }
    }
    if dirty {
        write_cursor(bridge_home, &cursor)?;
    }
    Ok(stats)
}

pub(crate) fn append_json_line(bus: &Path, value: &Value) -> io::Result<()> {
    let mut encoded = serde_json::to_vec(value).map_err(io::Error::other)?;
    encoded.push(b'\n');
    // A dedicated channel bus may precede its directory (W5); a receipt
    // must not vanish over a missing parent.
    if let Some(parent) = bus.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = shared_bus_file(bus)?;
    super::transcript_bus_maintenance::generation::append(bus, &file, &encoded)
}

/// One channel-session lifecycle receipt, before serialization.
pub(crate) struct ChannelSessionLine<'a> {
    pub state: &'a str,
    pub reason: &'a str,
    pub channel: &'a str,
    pub agent: &'a str,
    pub session_id: Option<&'a str>,
    pub autoseal_secs: u64,
    pub opened_at: SystemTime,
    pub utterance_silence_sec: f32,
    pub provider: Option<&'a str>,
    pub provider_session_id: Option<&'a str>,
}

pub(crate) fn channel_session_line(line: &ChannelSessionLine<'_>) -> Value {
    json!({
        "schema": CHANNEL_SESSION_SCHEMA,
        "kind": "channel_session",
        "state": line.state,
        "reason": line.reason,
        "channel": line.channel,
        "agent": line.agent,
        "session_id": line.session_id,
        "emitted_at": utc_now(),
        "autoseal_secs": line.autoseal_secs,
        "loud": line.state == "open",
        "opened_at": system_time_rfc3339(line.opened_at),
        "utterance_silence_sec": line.utterance_silence_sec,
        "provider": line.provider,
        "provider_session_id": line.provider_session_id,
    })
}

fn system_time_rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn cursor_path(bridge_home: &Path) -> PathBuf {
    bridge_home.join("runtime").join(CURSOR_FILE)
}

fn load_cursor(bridge_home: &Path) -> io::Result<Cursor> {
    let path = cursor_path(bridge_home);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Cursor::default()),
        Err(error) => return Err(error),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "agent ack cursor ignored");
            return Ok(Cursor::default());
        }
    };
    if value.get("schema").and_then(Value::as_str) != Some(CURSOR_SCHEMA) {
        return Ok(Cursor::default());
    }
    let bus_marks = value
        .get("bus_marks")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(path, mark)| {
                    Some((
                        path.clone(),
                        BusMark {
                            offset: mark.get("offset").and_then(Value::as_u64)?,
                            head_len: mark.get("head_len").and_then(Value::as_u64)? as u32,
                            head_hash: mark
                                .get("head_hash")
                                .and_then(Value::as_str)?
                                .parse()
                                .ok()?,
                            ino: mark.get("ino").and_then(Value::as_str)?.parse().ok()?,
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Cursor {
        emitted: string_set(value.get("emitted")),
        known_seals: string_set(value.get("known_seals")),
        ducked_replies: string_list(value.get("ducked_replies")),
        bus_marks,
    })
}

fn write_cursor(bridge_home: &Path, cursor: &Cursor) -> io::Result<()> {
    let path = cursor_path(bridge_home);
    let body = json!({
        "schema": CURSOR_SCHEMA,
        "emitted": cursor.emitted.iter().collect::<Vec<_>>(),
        "known_seals": cursor.known_seals.iter().collect::<Vec<_>>(),
        "ducked_replies": cursor.ducked_replies,
        "bus_marks": cursor
            .bus_marks
            .iter()
            .map(|(path, mark)| {
                (
                    path.clone(),
                    json!({
                        "offset": mark.offset,
                        "head_len": mark.head_len,
                        // u64 does not survive JSON floats; keep these textual.
                        "head_hash": mark.head_hash.to_string(),
                        "ino": mark.ino.to_string(),
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>(),
    });
    write_private_json(&path, &body)
}

fn write_private_json(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let temporary = path.with_extension("json.tmp");
    let mut encoded = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    encoded.push(b'\n');
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&encoded)?;
        file.sync_all()?;
    }
    fs::rename(temporary, path)
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    string_list(value).into_iter().collect()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

struct PendingDelivery {
    id: String,
    kind: Option<String>,
}

struct LeaseRecord {
    lease_id: String,
    provider: String,
    provider_session_id: String,
    bus: Option<PathBuf>,
    pending: Vec<PendingDelivery>,
}

fn load_leases(bridge_home: &Path) -> Vec<LeaseRecord> {
    let mut leases = Vec::new();
    let Ok(entries) = fs::read_dir(bridge_home.join("leases")) else {
        return leases;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get("schema").and_then(Value::as_str) != Some(LEASE_SCHEMA) {
            continue;
        }
        let Some(lease_id) = value.get("lease_id").and_then(Value::as_str) else {
            continue;
        };
        if !safe_lease_id(lease_id) {
            continue;
        }
        let Some(provider) = value
            .get("provider")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(provider_session_id) = value.get("provider_session_id").and_then(Value::as_str)
        else {
            continue;
        };
        let provider_session_id = provider_session_id.trim();
        if provider_session_id.is_empty() {
            continue;
        }
        let bus = value
            .get("bus")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|bus| !bus.is_empty())
            .map(PathBuf::from);
        let pending = value
            .get("pending")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item
                            .get("delivery_id")
                            .and_then(Value::as_str)
                            .filter(|id| is_delivery_id(id))?
                            .to_string();
                        let kind = item.get("kind").and_then(Value::as_str).map(str::to_string);
                        Some(PendingDelivery { id, kind })
                    })
                    .collect()
            })
            .unwrap_or_default();
        leases.push(LeaseRecord {
            lease_id: lease_id.to_string(),
            provider: provider.to_string(),
            provider_session_id: provider_session_id.to_string(),
            bus,
            pending,
        });
    }
    leases
}

fn load_channels(bridge_home: &Path) -> BTreeMap<String, ChannelBinding> {
    let mut channels = BTreeMap::new();
    let Ok(text) = fs::read_to_string(bridge_home.join(BINDING_FILENAME)) else {
        return channels;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return channels;
    };
    if value.get("schema").and_then(Value::as_str) != Some(BINDING_SCHEMA) {
        return channels;
    }
    let Some(bindings) = value.get("bindings").and_then(Value::as_object) else {
        return channels;
    };
    for (key, entry) in bindings {
        if key.len() != 1 || !key.bytes().all(|byte| byte.is_ascii_digit()) || key == "0" {
            continue;
        }
        let Some(audience) = entry.get("audience").and_then(Value::as_str) else {
            continue;
        };
        let Some(session) = entry.get("provider_session_id").and_then(Value::as_str) else {
            continue;
        };
        let audience = audience.trim();
        let session = session.trim();
        if audience.is_empty() || session.is_empty() {
            continue;
        }
        channels
            .entry(session.to_string())
            .or_insert(ChannelBinding {
                channel: key.clone(),
                audience: audience.to_string(),
            });
    }
    channels
}

/// Feed `on_line` every complete JSON line the bus gained since the mark,
/// then advance the mark past the last consumed newline. A rotated or
/// truncated file (shorter than the mark, different inode, or different
/// first bytes) resets the mark and replays from the start — the cursor's
/// emitted set keeps replays from double-appending. A trailing partial line
/// is left for the next pass.
///
/// Reads are chunked and each pass consumes at most `budget` bytes of
/// complete lines (always at least one complete line), so neither a 20 GB
/// backlog nor a rotation reset ever pulls the whole file into memory: the
/// 500 ms loop drains large histories a slice at a time.
const FOLD_CHUNK: usize = 4 << 20;
const FOLD_PASS_BUDGET: u64 = 64 << 20;
/// A single row larger than this is dropped unparsed. The scan only reads
/// acknowledgment metadata; no legitimate ack, seal or reply row approaches
/// this size, and an unbounded row must not hold the scan's memory hostage.
const FOLD_MAX_LINE: usize = 8 << 20;

/// The only bus-row fields the ack scan reads. Typed deserialization skips
/// building a JSON tree for every row: sampling on build 1452 placed 97% of
/// the scan in `from_str::<Value>` and 41% in memmove, almost all of it
/// materializing transcript payloads the scan never looks at.
#[derive(Default, serde::Deserialize)]
struct AckScanRow<'a> {
    #[serde(default, borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    status: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    state: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    delivery_id: Option<Cow<'a, str>>,
    #[serde(default)]
    spoken: Option<bool>,
    #[serde(default, borrow)]
    reply_id: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    emitted_at: Option<Cow<'a, str>>,
}

fn fold_bus_delta(
    path: &Path,
    mark: &mut BusMark,
    on_line: impl FnMut(&AckScanRow),
) -> io::Result<()> {
    fold_bus_delta_budgeted(path, mark, FOLD_PASS_BUDGET, on_line)
}

fn fold_bus_delta_budgeted(
    path: &Path,
    mark: &mut BusMark,
    budget: u64,
    mut on_line: impl FnMut(&AckScanRow),
) -> io::Result<()> {
    let mut file = match super::transcript_bus_maintenance::generation::Reader::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let len = file.len();
    let ino = if file.manifest.is_some() {
        file.identity().1
    } else {
        file_ino(&fs::metadata(path)?)
    };
    if mark.offset > 0 {
        let rotated = len < mark.offset || ino != mark.ino || {
            let mut head = vec![0_u8; mark.head_len as usize];
            file.read_exact(&mut head)?;
            hash_bytes(&head) != mark.head_hash
        };
        if rotated {
            *mark = BusMark::default();
        }
    }
    if len == mark.offset {
        return Ok(());
    }
    file.seek(SeekFrom::Start(mark.offset))?;

    let fresh = mark.offset == 0;
    let mut head_set = !fresh;
    let mut chunk = vec![0_u8; FOLD_CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut consumed: u64 = 0;
    let mut committed: u64 = 0;
    let mut storage = super::transcript_bus_maintenance::generation::Decoder::default();
    // Bytes of the current oversize row already dropped from `carry`. The
    // offset advances past them only once the row's newline lands, so a row
    // still being appended is never split.
    let mut oversize_dropped: u64 = 0;
    'passes: loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        carry.extend_from_slice(&chunk[..read]);
        if !head_set && (carry.len() >= 256 || carry.contains(&b'\n')) {
            let head_len = carry
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|newline| newline + 1)
                .unwrap_or(carry.len())
                .min(256);
            mark.head_len = head_len as u32;
            mark.head_hash = hash_bytes(&carry[..head_len]);
            mark.ino = ino;
            head_set = true;
        }
        loop {
            let Some(newline) = carry.iter().position(|byte| *byte == b'\n') else {
                if carry.len() > FOLD_MAX_LINE {
                    oversize_dropped += carry.len() as u64;
                    carry.clear();
                }
                break;
            };
            if oversize_dropped == 0 && newline <= FOLD_MAX_LINE {
                let line = &carry[..newline];
                if let Ok(line) = std::str::from_utf8(line) {
                    let line = line.trim();
                    if line.contains("codescribe.bus-chunk.v1")
                        || line.contains("persistence_encoding")
                        || storage.incomplete()
                    {
                        let value: Value = serde_json::from_str(line).map_err(io::Error::other)?;
                        for document in storage.push(value)? {
                            for value in
                                super::transcript_bus_maintenance::generation::rows(document)?
                            {
                                // Metadata borrows the expanded observation; never
                                // serialize its full document again for ACK folding.
                                let text = |key: &str| {
                                    value.get(key).and_then(Value::as_str).map(Cow::Borrowed)
                                };
                                let row = AckScanRow {
                                    kind: text("kind"),
                                    status: text("status"),
                                    state: text("state"),
                                    delivery_id: text("delivery_id"),
                                    spoken: value.get("spoken").and_then(Value::as_bool),
                                    reply_id: text("reply_id"),
                                    emitted_at: text("emitted_at"),
                                };
                                on_line(&row);
                            }
                        }
                    } else if !line.is_empty()
                        && (line.contains("\"delivery_id\"") || line.contains("\"agent_reply\""))
                        && let Ok(row) = serde_json::from_str::<AckScanRow>(line)
                    {
                        on_line(&row);
                    }
                }
            }
            carry.drain(..=newline);
            consumed += oversize_dropped + newline as u64 + 1;
            oversize_dropped = 0;
            if !storage.incomplete() {
                committed = consumed;
            }
            // Budget bounds one pass, never a single line: with at least one
            // line consumed the mark advances and the next pass continues.
            if consumed >= budget && !storage.incomplete() {
                break 'passes;
            }
        }
    }
    mark.offset += committed;
    Ok(())
}

fn file_ino(metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.ino()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

struct AckMarker {
    delivery_id: String,
    recipient: Option<ChannelBinding>,
    has_envelope: bool,
}

fn marker_delivery_id(path: &Path) -> Option<&str> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".json")?;
    is_delivery_id(stem).then_some(stem)
}

fn read_marker(
    path: &Path,
    lease: &LeaseRecord,
    fallback_bus: &Path,
) -> io::Result<Option<AckMarker>> {
    let Some(stem) = marker_delivery_id(path) else {
        return Ok(None);
    };
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 1 << 20 {
        return Ok(None);
    }
    let file = fs::File::open(path)?;
    let value: Value = match serde_json::from_reader(file.take(1 << 20)) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("lease_id").and_then(Value::as_str) != Some(lease.lease_id.as_str()) {
        return Ok(None);
    }
    if object.get("delivery_id").and_then(Value::as_str) != Some(stem) {
        return Ok(None);
    }
    let Some(envelope) = object.get("envelope") else {
        return Ok((object.len() == 2).then(|| AckMarker {
            delivery_id: stem.to_string(),
            recipient: None,
            has_envelope: false,
        }));
    };
    let bus = lease.bus.as_deref().unwrap_or(fallback_bus);
    let matches_owner = |value: &Value| {
        value["lease_id"].as_str() == Some(lease.lease_id.as_str())
            && value["provider"].as_str() == Some(lease.provider.as_str())
            && value["provider_session_id"].as_str() == Some(lease.provider_session_id.as_str())
            && value["bus"].as_str() == bus.to_str()
    };
    if !matches_owner(&value)
        || !matches_owner(envelope)
        || envelope["delivery_id"].as_str() != Some(stem)
        || !matches!(envelope["kind"].as_str(), Some("seal" | "message"))
    {
        return Ok(None);
    }
    if envelope["kind"] == "message"
        && (envelope["producer_schema"] != "codescribe.agent-user-message.v1"
            || envelope["source"] != "typed"
            || envelope["message_id"]
                .as_str()
                .is_none_or(|id| !is_delivery_id(id))
            || envelope["source_event_id"] != envelope["message_id"])
    {
        return Ok(None);
    }
    let recipient = envelope["recipients"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| matches_owner(entry)))
        .and_then(|entry| {
            let channel = match &entry["channel"] {
                Value::String(channel) => channel.clone(),
                Value::Number(channel) => channel.to_string(),
                _ => return None,
            };
            let audience = entry["name"]
                .as_str()
                .or_else(|| entry["audience"].as_str())?;
            if channel.len() != 1
                || !channel.bytes().all(|digit| (b'1'..=b'9').contains(&digit))
                || audience.trim().is_empty()
            {
                return None;
            }
            Some(ChannelBinding {
                channel,
                audience: audience.to_string(),
            })
        });
    Ok(Some(AckMarker {
        delivery_id: stem.to_string(),
        recipient,
        has_envelope: true,
    }))
}

fn delivery_id_of<'a>(row: &'a AckScanRow<'_>) -> Option<&'a str> {
    row.delivery_id.as_deref().filter(|id| is_delivery_id(id))
}

fn is_delivery_id(value: &str) -> bool {
    value.len() == 24
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn safe_lease_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The delivery id of a bus row that proves a seal, if this row is one.
fn seal_delivery_id<'a>(row: &'a AckScanRow<'_>) -> Option<&'a str> {
    let sealed =
        row.kind.as_deref() == Some("seal") || row.status.as_deref() == Some("transcript_sealed");
    if sealed { delivery_id_of(row) } else { None }
}

fn spoken_reply_id(row: &AckScanRow<'_>) -> Option<String> {
    if !matches!(
        row.kind.as_deref(),
        Some("agent_reply" | "agent_reply_playback")
    ) || (row.kind.as_deref() == Some("agent_reply_playback")
        && row.state.as_deref() != Some("spoken"))
    {
        return None;
    }
    if row.spoken != Some(true) {
        return None;
    }
    row.reply_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            row.emitted_at
                .as_deref()
                .map(|emitted| format!("emitted:{emitted}"))
        })
}

fn reply_is_recent(row: &AckScanRow<'_>) -> bool {
    let Some(stamp) = row.emitted_at.as_deref() else {
        return true;
    };
    let Ok(emitted) = chrono::DateTime::parse_from_rfc3339(stamp) else {
        return true;
    };
    let age = Utc::now().signed_duration_since(emitted).num_seconds();
    (-2..5).contains(&age)
}

fn remember_reply(cursor: &mut Cursor, dirty: &mut bool, reply_id: &str) {
    if cursor.ducked_replies.iter().any(|known| known == reply_id) {
        return;
    }
    cursor.ducked_replies.push(reply_id.to_string());
    if cursor.ducked_replies.len() > 256 {
        let overflow = cursor.ducked_replies.len() - 256;
        cursor.ducked_replies.drain(0..overflow);
    }
    *dirty = true;
}

fn note_spoken_reply(cursor: &mut Cursor, dirty: &mut bool, reply_id: &str) {
    if cursor.ducked_replies.iter().any(|known| known == reply_id) {
        return;
    }
    crate::audio::tts_duck::observe_spoken_reply(true);
    remember_reply(cursor, dirty, reply_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    const SEAL_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const DRAFT_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const LEASE_ID: &str = "0123456789abcdef0123456789abcdef";

    struct QuietDuck;
    impl Drop for QuietDuck {
        fn drop(&mut self) {
            crate::audio::tts_duck::clear();
        }
    }

    fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn binding(session: &str) -> Value {
        json!({
            "schema": BINDING_SCHEMA,
            "bindings": {
                "2": {
                    "audience": "Roman",
                    "provider": "codex",
                    "provider_session_id": session
                }
            }
        })
    }

    fn lease(bus: &Path, pending: Value) -> Value {
        json!({
            "schema": LEASE_SCHEMA,
            "lease_id": LEASE_ID,
            "provider": "codex",
            "provider_session_id": "sess-roman",
            "name": "roman",
            "bus": bus,
            "pending": pending
        })
    }

    fn marker() -> Value {
        json!({"lease_id": LEASE_ID, "delivery_id": SEAL_ID})
    }

    fn ack_rows(bus: &Path) -> Vec<Value> {
        crate::durable_bus_oracle::logical_text(bus)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|value: &Value| value.get("kind").and_then(Value::as_str) == Some("agent_ack"))
            .collect()
    }

    #[test]
    fn owned_envelope_marker_retains_original_channel_after_rebind() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("bus.jsonl");
        write_json(
            &bridge.join(BINDING_FILENAME),
            &binding("replacement-session"),
        );
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(&bus, json!([{ "delivery_id": SEAL_ID, "kind": "seal" }])),
        );
        let marker = json!({"lease_id": LEASE_ID, "delivery_id": SEAL_ID,
            "provider": "codex", "provider_session_id": "sess-roman", "bus": bus,
            "envelope": {"delivery_id": SEAL_ID, "lease_id": LEASE_ID,
                "provider": "codex", "provider_session_id": "sess-roman", "bus": bus,
                "kind": "seal", "recipients": [{"channel": "2", "name": "Roman",
                    "provider": "codex", "provider_session_id": "sess-roman",
                    "lease_id": LEASE_ID, "bus": bus}]}});
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &marker,
        );
        let result = scan(&bridge, &bus).unwrap();
        assert_eq!(result.appended, 1);
        let rows = ack_rows(&bus);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["agent"], "Roman");
        assert_eq!(rows[0]["channel"], "2");
        assert_eq!(rows[0]["provider"], "codex");
        assert_eq!(rows[0]["provider_session_id"], "sess-roman");
        assert_eq!(rows[0]["lease_id"], LEASE_ID);
    }

    #[test]
    fn typed_message_ack_preserves_owned_recipient_after_rebind() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("bus.jsonl");
        write_json(
            &bridge.join(BINDING_FILENAME),
            &binding("replacement-session"),
        );
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(&bus, json!([{ "delivery_id": SEAL_ID, "kind": "message" }])),
        );
        let marker = json!({"lease_id": LEASE_ID, "delivery_id": SEAL_ID,
            "provider":"codex", "provider_session_id":"sess-roman", "bus":bus,
            "envelope":{"delivery_id":SEAL_ID, "lease_id":LEASE_ID,
                "provider":"codex", "provider_session_id":"sess-roman", "bus":bus,
                "kind":"message", "producer_schema":"codescribe.agent-user-message.v1",
                "source":"typed", "message_id":"111111111111111111111111",
                "source_event_id":"111111111111111111111111",
                "recipients":[{"channel":"2", "name":"Roman", "provider":"codex",
                    "provider_session_id":"sess-roman", "lease_id":LEASE_ID, "bus":bus}]}});
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &marker,
        );
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        let rows = ack_rows(&bus);
        assert_eq!(rows[0]["agent"], "Roman");
        assert_eq!(rows[0]["channel"], "2");
    }

    #[test]
    fn playback_completion_has_spoken_identity_but_text_does_not() {
        let completion: AckScanRow<'_> = serde_json::from_str(
            r#"{"kind":"agent_reply_playback","state":"spoken","spoken":true,"reply_id":"reply-1"}"#).unwrap();
        assert_eq!(spoken_reply_id(&completion).as_deref(), Some("reply-1"));
        let failed: AckScanRow<'_> = serde_json::from_str(
            r#"{"kind":"agent_reply_playback","state":"failed","spoken":false,"reply_id":"reply-1"}"#).unwrap();
        assert!(spoken_reply_id(&failed).is_none());
        let text: AckScanRow<'_> =
            serde_json::from_str(r#"{"kind":"agent_reply","spoken":false,"reply_id":"reply-1"}"#)
                .unwrap();
        assert!(spoken_reply_id(&text).is_none());
    }

    #[test]
    fn seal_marker_appends_one_row_and_a_restart_does_not_duplicate_it() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("transcript-events.jsonl");
        let other = root.path().join("other.jsonl");
        fs::write(
            &bus,
            "{\"schema\":\"codescribe.transcript.v1\",\"status\":\"listening\",\"text\":\"keep\"}\n",
        )
        .unwrap();
        let kept = fs::read_to_string(&bus).unwrap();
        write_json(&bridge.join(BINDING_FILENAME), &binding("sess-roman"));
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(&bus, json!([{ "delivery_id": SEAL_ID, "kind": "seal" }])),
        );
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &marker(),
        );

        let first = scan(&bridge, &other).unwrap();
        assert_eq!(first.appended, 1, "{first:?}");
        let second = scan(&bridge, &other).unwrap();
        assert_eq!(second.appended, 0, "{second:?}");
        fs::remove_file(cursor_path(&bridge)).unwrap();
        let restarted = scan(&bridge, &other).unwrap();
        assert_eq!(restarted.appended, 0, "{restarted:?}");

        let text = crate::durable_bus_oracle::logical_text(&bus);
        assert!(
            text.starts_with(&kept),
            "existing bus rows must stay put:\n{text}"
        );
        let rows = ack_rows(&bus);
        assert_eq!(rows.len(), 1, "{text}");
        assert!(!other.exists());
        let row = &rows[0];
        assert_eq!(row.get("schema").and_then(Value::as_str), Some(ACK_SCHEMA));
        assert_eq!(row.get("kind").and_then(Value::as_str), Some("agent_ack"));
        assert_eq!(row.get("channel").and_then(Value::as_str), Some("2"));
        assert_eq!(
            row.get("delivery_id").and_then(Value::as_str),
            Some(SEAL_ID)
        );
        assert_eq!(row.get("agent").and_then(Value::as_str), Some("Roman"));
        let emitted_at = row.get("emitted_at").and_then(Value::as_str).unwrap();
        assert!(emitted_at.ends_with('Z'), "{emitted_at}");
        assert!(cursor_path(&bridge).starts_with(bridge.join("runtime")));
        assert!(!cursor_path(&bridge).starts_with(bridge.join("acknowledgments")));
    }

    #[test]
    fn a_seal_seen_before_the_marker_still_emits_after_pending_is_swept() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("bus.jsonl");
        write_json(&bridge.join(BINDING_FILENAME), &binding("sess-roman"));
        let lease_path = bridge.join("leases").join(format!("{LEASE_ID}.json"));
        write_json(
            &lease_path,
            &lease(&bus, json!([{ "delivery_id": SEAL_ID, "kind": "seal" }])),
        );
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        write_json(&lease_path, &lease(&bus, json!([])));
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &marker(),
        );
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
        assert_eq!(ack_rows(&bus).len(), 1);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        assert_eq!(ack_rows(&bus).len(), 1);
    }

    #[test]
    fn a_bus_seal_line_proves_the_delivery_after_pending_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("bus.jsonl");
        fs::write(
            &bus,
            format!(
                "{}\n",
                json!({
                    "schema": "codescribe.transcript.v1",
                    "status": "transcript_sealed",
                    "delivery_id": SEAL_ID,
                    "text": "sealed"
                })
            ),
        )
        .unwrap();
        write_json(&bridge.join(BINDING_FILENAME), &binding("sess-roman"));
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(&bus, json!([])),
        );
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &marker(),
        );
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
        assert_eq!(ack_rows(&bus).len(), 1);
    }

    #[test]
    fn draft_and_foreign_markers_do_not_emit() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        let bus = root.path().join("bus.jsonl");
        write_json(&bridge.join(BINDING_FILENAME), &binding("sess-roman"));
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(&bus, json!([{ "delivery_id": DRAFT_ID, "kind": "draft" }])),
        );
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{DRAFT_ID}.json")),
            &json!({"lease_id": LEASE_ID, "delivery_id": DRAFT_ID}),
        );
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{SEAL_ID}.json")),
            &json!({"lease_id": LEASE_ID, "delivery_id": SEAL_ID, "extra": true}),
        );
        let stats = scan(&bridge, &bus).unwrap();
        assert_eq!(stats.appended, 0, "{stats:?}");
        assert_eq!(stats.skipped_unproven, 1, "{stats:?}");
        assert!(ack_rows(&bus).is_empty());
    }

    #[test]
    #[serial(agent_ack_duck)]
    fn a_spoken_reply_arms_the_channel_gate_once() {
        let _quiet = QuietDuck;
        crate::audio::tts_duck::clear();
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("bridge");
        fs::create_dir_all(&bridge).unwrap();
        let bus = root.path().join("bus.jsonl");
        fs::write(
            &bus,
            format!(
                "{}\n{}\n",
                json!({
                    "schema": "codescribe.agent-reply.v1",
                    "kind": "agent_reply",
                    "spoken": false,
                    "reply_id": "silent-reply",
                    "text": "quiet"
                }),
                json!({
                    "schema": "codescribe.agent-reply.v1",
                    "kind": "agent_reply",
                    "spoken": true,
                    "reply_id": "spoken-reply",
                    "text": "hello"
                })
            ),
        )
        .unwrap();
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        assert!(crate::audio::tts_duck::channel_capture_should_drop());
        crate::audio::tts_duck::clear();
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        assert!(
            !crate::audio::tts_duck::channel_capture_should_drop(),
            "the same spoken reply must not re-arm the gate on the next scan"
        );
    }

    const HIDDEN_ID: &str = "cccccccccccccccccccccccc";
    const DELTA_ID: &str = "dddddddddddddddddddddddd";
    const ROTATED_ID: &str = "eeeeeeeeeeeeeeeeeeeeeeee";
    const PARTIAL_ID: &str = "ffffffffffffffffffffffff";

    fn seal_row(id: &str) -> String {
        format!(
            "{}\n",
            json!({"schema": "codescribe.transcript.v1", "kind": "seal", "delivery_id": id})
        )
    }

    fn marker_for(bridge: &Path, id: &str) {
        write_json(
            &bridge
                .join("acknowledgments")
                .join(LEASE_ID)
                .join(format!("{id}.json")),
            &json!({"lease_id": LEASE_ID, "delivery_id": id}),
        );
    }

    fn bridge_with_lease(root: &Path, bus: &Path) -> PathBuf {
        let bridge = root.join("bridge");
        write_json(&bridge.join(BINDING_FILENAME), &binding("sess-roman"));
        write_json(
            &bridge.join("leases").join(format!("{LEASE_ID}.json")),
            &lease(bus, json!([])),
        );
        bridge
    }

    #[test]
    fn a_scan_never_rereads_consumed_bus_history() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let bridge = bridge_with_lease(root.path(), &bus);
        // The first line is longer than the 256-byte head fingerprint, so the
        // second line lies in consumed-but-unfingerprinted territory.
        let first_line = format!(
            "{}\n",
            json!({
                "schema": "codescribe.transcript.v1",
                "kind": "seal",
                "delivery_id": SEAL_ID,
                "text": "x".repeat(300)
            })
        );
        let second_line = seal_row("9999999999999999999999aa");
        fs::write(&bus, format!("{first_line}{second_line}")).unwrap();
        marker_for(&bridge, SEAL_ID);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);

        // Rewrite the CONSUMED second line in place with a same-length, fully
        // valid seal row for another delivery. An implementation that re-reads
        // history would prove and emit it; the mark must never look back.
        let hidden = seal_row(HIDDEN_ID);
        assert_eq!(
            hidden.len(),
            second_line.len(),
            "witness must keep the byte length"
        );
        // The already-consumed prefix is now the preserved undated generation.
        // Mutate that same physical inode, not the new hot suffix.
        let manifest: Value = serde_json::from_slice(
            &fs::read(format!("{}.generations.json", bus.display())).unwrap(),
        )
        .unwrap();
        let consumed = PathBuf::from(manifest["segments"][0]["path"].as_str().unwrap());
        let mut bytes = fs::read(&consumed).unwrap();
        let start = first_line.len();
        bytes[start..start + hidden.len()].copy_from_slice(hidden.as_bytes());
        fs::write(&consumed, &bytes).unwrap();
        marker_for(&bridge, HIDDEN_ID);
        assert_eq!(
            scan(&bridge, &bus).unwrap().appended,
            0,
            "history was re-read"
        );

        // The delta after the offset is still consumed normally.
        let mut file = OpenOptions::new().append(true).open(&bus).unwrap();
        file.write_all(seal_row(DELTA_ID).as_bytes()).unwrap();
        drop(file);
        marker_for(&bridge, DELTA_ID);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
        let ids: Vec<String> = ack_rows(&bus)
            .iter()
            .filter_map(|row| {
                row.get("delivery_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert!(ids.contains(&SEAL_ID.to_string()) && ids.contains(&DELTA_ID.to_string()));
        assert!(!ids.contains(&HIDDEN_ID.to_string()), "{ids:?}");
    }

    #[test]
    fn bus_truncation_resets_the_offset_and_replays_without_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let bridge = bridge_with_lease(root.path(), &bus);
        fs::write(&bus, seal_row(SEAL_ID)).unwrap();
        marker_for(&bridge, SEAL_ID);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);

        // Rotation: the whole file is replaced (same length as before, same
        // inode - only the head fingerprint can tell). The mark resets, the
        // fresh seal is emitted, and the already-emitted delivery is not
        // appended again even though the replay sees no trace of it on disk.
        fs::write(&bus, seal_row(ROTATED_ID)).unwrap();
        marker_for(&bridge, ROTATED_ID);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 0);
        let rows = ack_rows(&bus);
        assert_eq!(rows.len(), 1, "rotation destroyed the old rows on disk");
        assert_eq!(
            rows[0].get("delivery_id").and_then(Value::as_str),
            Some(ROTATED_ID)
        );
    }

    #[test]
    fn a_partial_bus_line_waits_for_its_newline() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let bridge = bridge_with_lease(root.path(), &bus);
        let row = seal_row(PARTIAL_ID);
        let (head, tail) = row.split_at(row.len() - 9);
        fs::write(&bus, head).unwrap();
        marker_for(&bridge, PARTIAL_ID);
        assert_eq!(
            scan(&bridge, &bus).unwrap().appended,
            0,
            "half a line proved a seal"
        );

        let mut file = OpenOptions::new().append(true).open(&bus).unwrap();
        file.write_all(tail.as_bytes()).unwrap();
        drop(file);
        assert_eq!(scan(&bridge, &bus).unwrap().appended, 1);
    }

    #[test]
    fn a_pass_budget_drains_a_backlog_across_scans() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let rows: String = (0..40).map(|_| seal_row(SEAL_ID)).collect();
        fs::write(&bus, &rows).unwrap();
        let line_len = seal_row(SEAL_ID).len() as u64;

        let mut mark = BusMark::default();
        let mut seen = 0_usize;
        // Budget below one line still consumes exactly one complete line.
        fold_bus_delta_budgeted(&bus, &mut mark, 1, |_| seen += 1).unwrap();
        assert_eq!(seen, 1);
        assert_eq!(mark.offset, line_len);
        // A three-line budget takes three more; nothing is re-read.
        fold_bus_delta_budgeted(&bus, &mut mark, line_len * 3, |_| seen += 1).unwrap();
        assert_eq!(seen, 4);
        assert_eq!(mark.offset, line_len * 4);
        // A large budget drains the rest in one pass.
        fold_bus_delta_budgeted(&bus, &mut mark, u64::MAX, |_| seen += 1).unwrap();
        assert_eq!(seen, 40);
        assert_eq!(mark.offset, line_len * 40);
    }

    #[test]
    fn an_oversize_row_is_dropped_and_the_scan_moves_past_it() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let pad = "x".repeat(FOLD_MAX_LINE + 1024);
        let oversize = format!(
            "{}\n",
            json!({"schema": "codescribe.transcript.v1", "kind": "seal", "delivery_id": HIDDEN_ID, "pad": pad})
        );
        let normal = seal_row(SEAL_ID);
        fs::write(&bus, format!("{oversize}{normal}")).unwrap();

        let mut mark = BusMark::default();
        let mut seen = Vec::new();
        fold_bus_delta(&bus, &mut mark, |row| {
            seen.push(row.delivery_id.clone().map(Cow::into_owned));
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![Some(SEAL_ID.to_string())],
            "the oversize row is dropped unparsed; the next row is read"
        );
        assert_eq!(
            mark.offset,
            (oversize.len() + normal.len()) as u64,
            "the offset advances past the dropped row"
        );

        // An oversize row still missing its newline holds the offset: the
        // writer may still be appending it, and the scan must not split it.
        let mut file = OpenOptions::new().append(true).open(&bus).unwrap();
        file.write_all("y".repeat(FOLD_MAX_LINE + 1024).as_bytes())
            .unwrap();
        drop(file);
        let before = mark.offset;
        let mut extra = 0_usize;
        fold_bus_delta(&bus, &mut mark, |_| extra += 1).unwrap();
        assert_eq!(extra, 0);
        assert_eq!(
            mark.offset, before,
            "a rowless tail never advances the mark"
        );
    }

    #[test]
    fn a_rename_rotation_with_an_identical_head_is_still_detected() {
        let root = tempfile::tempdir().unwrap();
        let bus = root.path().join("bus.jsonl");
        let first_line = seal_row(SEAL_ID);
        fs::write(&bus, &first_line).unwrap();
        let mut mark = BusMark::default();
        let mut seen = 0_usize;
        fold_bus_delta(&bus, &mut mark, |_| seen += 1).unwrap();
        assert_eq!(seen, 1);

        // A new, LONGER file with a byte-identical head replaces the bus via
        // rename: length and fingerprint both match, only the inode differs.
        let staged = root.path().join("bus.jsonl.new");
        fs::write(&staged, format!("{first_line}{}", seal_row(ROTATED_ID))).unwrap();
        fs::rename(&staged, &bus).unwrap();
        let mut replayed = 0_usize;
        fold_bus_delta(&bus, &mut mark, |_| replayed += 1).unwrap();
        assert_eq!(replayed, 2, "the rename rotation went unnoticed");
        assert_eq!(
            mark.offset,
            (first_line.len() + seal_row(ROTATED_ID).len()) as u64
        );
    }
}
