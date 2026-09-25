//! Retention for the clean transcript bus.
//!
//! WHY THIS EXISTS. `transcript-events.jsonl` is append-only with no bound. On
//! 2026-09-18 it held 464 MB spanning 33 days — about 14 MB a day, or 170 GB a
//! year if nothing intervenes. The composition explains the rate: over the last
//! 20 MB, 92.7% of the bytes were `codescribe.transcript-evidence.v1` and 7.3%
//! were `codescribe.transcript.v1`.
//!
//! That ratio is the whole design problem. One file serves two roles with
//! opposite retention needs:
//!
//! - the DELIVERY record (`transcript.v1`) is small, and an operator may want
//!   to find a take from months ago. It is never dropped here.
//! - the EVIDENCE record (`transcript-evidence.v1`) is large and is consumed by
//!   the quality loop within days of being written. Past that window it is
//!   paying rent in gigabytes.
//!
//! So compaction is by schema and age, not by truncation: every delivery row
//! survives regardless of age, evidence older than the retention window is
//! dropped, and any row this code cannot parse is kept — an unknown schema is
//! not permission to discard someone's data.
//!
//! SAFETY. In-app compaction stages outside the descriptor lock, then copies
//! the appended tail and replaces the descriptor under that lock. The separate CLI process
//! cannot take that lock, so its command retains the quiet-period refusal and
//! re-checks file identity before rename.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};

/// Size past which the bus is worth compacting. Below this the reclaim is not
/// worth the risk of rewriting a file another process may be appending to.
pub const COMPACTION_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;

/// Default evidence retention. Deliberately generous: the cost of keeping
/// evidence a few days too long is disk, and the cost of dropping it a day too
/// early is a diagnosis that can no longer be made. `status` prints what
/// shorter windows would reclaim so the operator can choose a sharper one.
pub const DEFAULT_EVIDENCE_RETENTION_DAYS: u32 = 14;

/// Read the canonical Settings value for a new compaction pass.
pub fn evidence_retention_days() -> u32 {
    codescribe_core::config::UserSettings::load()
        .evidence_retention_days
        .unwrap_or(DEFAULT_EVIDENCE_RETENTION_DAYS)
        .clamp(1, 3650)
}

/// Windows offered in the status preview, in days.
const PREVIEW_WINDOWS: [u32; 4] = [3, 7, 14, 30];
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
static LAST_APP_CHECK: OnceLock<Mutex<HashMap<PathBuf, (Instant, u32)>>> = OnceLock::new();
static ACTIVE_APP_COMPACTIONS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// A write this recent means a session may still be open. Compaction refuses
/// rather than race an append descriptor it cannot see.
const QUIET_PERIOD: Duration = Duration::from_secs(60);

/// Schema of a row that carries the delivered transcript. Never dropped.
const DELIVERY_SCHEMA: &str = "codescribe.transcript.v1";
/// Schema of a row that carries acoustic evidence. Dropped once it ages out.
const EVIDENCE_SCHEMA: &str = "codescribe.transcript-evidence.v1";

/// What the bus looks like right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusStatus {
    /// Where the bus lives.
    pub path: PathBuf,
    /// Total size on disk.
    pub bytes: u64,
    /// Rows carrying a delivered transcript.
    pub delivery_rows: u64,
    /// Bytes those rows occupy.
    pub delivery_bytes: u64,
    /// Rows carrying acoustic evidence.
    pub evidence_rows: u64,
    /// Bytes those rows occupy.
    pub evidence_bytes: u64,
    /// Rows of any other or unparseable shape; compaction always keeps these.
    pub other_rows: u64,
    /// Timestamp of the first row that carries one.
    pub first_seen: Option<String>,
    /// Timestamp of the last row that carries one.
    pub last_seen: Option<String>,
    /// For each candidate window, the evidence bytes it would drop.
    ///
    /// Evidence arrives in bursts tied to sessions, not evenly by calendar day
    /// — on 2026-09-18 the bus held zero evidence for its first eleven days and
    /// 90 MiB on a single later one. A fixed default would therefore reclaim
    /// wildly different amounts from one week to the next, so `status` shows
    /// the trade and the operator picks the window.
    pub retention_preview: Vec<(u32, u64)>,
}

impl BusStatus {
    /// True when the bus has grown past the point where compaction pays.
    pub fn wants_compaction(&self) -> bool {
        self.bytes >= COMPACTION_THRESHOLD_BYTES
    }
}

/// What a compaction did, or would do under `dry_run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    /// Rows read from the original file.
    pub rows_read: u64,
    /// Rows written to the compacted file.
    pub rows_kept: u64,
    /// Evidence rows older than the cutoff that were dropped.
    pub evidence_rows_dropped: u64,
    /// Size before.
    pub bytes_before: u64,
    /// Size after, or the size the rewrite would have produced.
    pub bytes_after: u64,
    /// False when `dry_run` kept the original in place.
    pub applied: bool,
    /// Time spent holding the app's shared writer lock during the final swap.
    pub locked_ms: u64,
    /// Complete tail rows read while holding the shared writer lock.
    pub locked_rows_read: u64,
    /// Tail bytes read while holding the shared writer lock.
    pub locked_bytes_read: u64,
}

impl CompactionReport {
    /// Bytes the compaction reclaimed, saturating at zero.
    pub fn bytes_reclaimed(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }
}

/// Read a row's schema and timestamp without deserializing the whole payload.
///
/// Evidence rows are kilobytes of nested receipts; parsing every one into a
/// typed structure to read two fields would make a status pass cost more than
/// the compaction it is deciding about.
fn row_schema_and_time(line: &str) -> (Option<String>, Option<String>, bool) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return (None, None, false);
    };
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let time = ["emitted_at", "ts", "timestamp", "created_at"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string);
    let retained_terminal_edit = value.get("reducer_action").and_then(|v| v.as_str())
        == Some("apply_manual_edit")
        && value.get("terminal").and_then(|v| v.as_bool()) == Some(true);
    (schema, time, retained_terminal_edit)
}

/// Summarise the bus: size, composition and span.
pub fn bus_status(path: &Path) -> Result<BusStatus> {
    bus_status_with_retention(path, None)
}

fn bus_status_with_retention(path: &Path, retention_days: Option<u32>) -> Result<BusStatus> {
    bus_status_with_retention_limit(path, retention_days, None)
}

fn bus_status_with_retention_limit(
    path: &Path,
    retention_days: Option<u32>,
    limit: Option<u64>,
) -> Result<BusStatus> {
    let bytes = limit.unwrap_or_else(|| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));
    let mut status = BusStatus {
        path: path.to_path_buf(),
        bytes,
        delivery_rows: 0,
        delivery_bytes: 0,
        evidence_rows: 0,
        evidence_bytes: 0,
        other_rows: 0,
        first_seen: None,
        last_seen: None,
        retention_preview: Vec::new(),
    };
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- bus path comes from `transcript_bus_path()` or a test fixture, never from a request; this is a desktop CLI with no network input surface.
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(status),
        Err(error) => return Err(error).with_context(|| format!("open bus {}", path.display())),
    };
    let mut windows = PREVIEW_WINDOWS.to_vec();
    if let Some(days) = retention_days
        && !windows.contains(&days)
    {
        windows.push(days);
    }
    let cutoffs: Vec<(u32, String)> = windows
        .iter()
        .map(|days| (*days, cutoff_for(*days)))
        .collect();
    let mut reclaimable = vec![0u64; cutoffs.len()];
    for line in BufReader::new(file.take(bytes)).lines() {
        let line = line.with_context(|| format!("read bus {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let width = line.len() as u64 + 1;
        let (schema, time, retained_terminal_edit) = row_schema_and_time(&line);
        match schema.as_deref() {
            Some(DELIVERY_SCHEMA) => {
                status.delivery_rows += 1;
                status.delivery_bytes += width;
            }
            Some(EVIDENCE_SCHEMA) => {
                status.evidence_rows += 1;
                status.evidence_bytes += width;
                if let Some(time) = time.as_deref()
                    && !retained_terminal_edit
                {
                    for (index, (_, cutoff)) in cutoffs.iter().enumerate() {
                        if time < cutoff.as_str() {
                            reclaimable[index] += width;
                        }
                    }
                }
            }
            _ => status.other_rows += 1,
        }
        if let Some(time) = time {
            if status.first_seen.is_none() {
                status.first_seen = Some(time.clone());
            }
            status.last_seen = Some(time);
        }
    }
    status.retention_preview = cutoffs
        .iter()
        .map(|(days, _)| *days)
        .zip(reclaimable)
        .collect();
    Ok(status)
}

/// RFC 3339 instant `days` before now, in the form the rows carry.
fn cutoff_for(days: u32) -> String {
    (chrono::Utc::now() - chrono::Duration::days(i64::from(days)))
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Drop evidence rows older than `retention_days`, keeping everything else.
///
/// Returns `Ok` with `applied: false` under `dry_run`, and an error when the
/// bus looks live — see the safety note in the module docs.
pub fn compact_bus(path: &Path, retention_days: u32, dry_run: bool) -> Result<CompactionReport> {
    compact_bus_inner(path, retention_days, dry_run, false, None)
}

/// Compact through the app's shared writer. Appends wait only for tail copy
/// and descriptor replacement.
pub fn compact_bus_owned(
    path: &Path,
    retention_days: u32,
    trigger: &str,
) -> Result<Option<CompactionReport>> {
    let active = ACTIVE_APP_COMPACTIONS.get_or_init(|| Mutex::new(HashSet::new()));
    {
        let mut active = active.lock().unwrap_or_else(|error| error.into_inner());
        if !active.insert(path.to_path_buf()) {
            return Ok(None);
        }
    }
    let result = compact_bus_owned_reserved(path, retention_days, trigger);
    active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(path);
    result
}

fn compact_bus_owned_reserved(
    path: &Path,
    retention_days: u32,
    trigger: &str,
) -> Result<Option<CompactionReport>> {
    let checks = LAST_APP_CHECK.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let mut checks = checks.lock().unwrap_or_else(|error| error.into_inner());
        if trigger == "idle"
            && checks.get(path).is_some_and(|(checked, days)| {
                *days == retention_days && checked.elapsed() < IDLE_CHECK_INTERVAL
            })
        {
            return Ok(None);
        }
        // Reserve this check before a second idle task can queue behind the
        // writer. A failed pass clears the reservation below.
        checks.insert(path.to_path_buf(), (Instant::now(), retention_days));
    }
    let result = compact_bus_owned_once(
        path,
        retention_days,
        trigger,
        COMPACTION_THRESHOLD_BYTES,
        || {},
    );
    if result.is_err() {
        checks
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(path);
    }
    result
}

fn compact_bus_owned_once(
    path: &Path,
    retention_days: u32,
    trigger: &str,
    minimum_bytes: u64,
    before_swap: impl FnOnce(),
) -> Result<Option<CompactionReport>> {
    let shared = super::transcript_bus::shared_bus_file(path)?;
    let initial = {
        let file = shared.lock().unwrap_or_else(|error| error.into_inner());
        file.metadata()?
    };
    if initial.len() < minimum_bytes {
        return Ok(None);
    }
    let status = bus_status_with_retention_limit(path, Some(retention_days), Some(initial.len()))?;
    if status.bytes < minimum_bytes
        || status
            .retention_preview
            .iter()
            .find(|(days, _)| *days == retention_days)
            .is_none_or(|(_, bytes)| *bytes == 0)
    {
        return Ok(None);
    }
    let (mut report, staged) = stage_bus(path, retention_days, initial.len(), true)?;
    if report.evidence_rows_dropped == 0 {
        std::fs::remove_file(&staged).ok();
        return Ok(None);
    }
    before_swap();
    let result = (|| -> Result<u64> {
        use std::os::unix::fs::MetadataExt;
        let mut writer = shared.lock().unwrap_or_else(|error| error.into_inner());
        let locked_at = Instant::now();
        let visible = std::fs::metadata(path)?;
        let descriptor = writer.metadata()?;
        anyhow::ensure!(
            visible.dev() == initial.dev()
                && visible.ino() == initial.ino()
                && descriptor.dev() == initial.dev()
                && descriptor.ino() == initial.ino()
                && visible.len() >= initial.len()
                && descriptor.len() == visible.len(),
            "transcript bus shrank or was replaced during owned compaction"
        );
        let tail_len = visible.len() - initial.len();
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- this is the app-owned Bus path already verified against its shared writer descriptor, or a test temporary path.
        let mut source = std::fs::File::open(path)?;
        source.seek(SeekFrom::Start(initial.len()))?;
        let mut tail = source.take(tail_len);
        let mut out = std::fs::OpenOptions::new().append(true).open(&staged)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = tail.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let rows = buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64;
            report.rows_read += rows;
            report.rows_kept += rows;
            report.locked_rows_read += rows;
            report.locked_bytes_read += read as u64;
            out.write_all(&buffer[..read])?;
        }
        out.flush()?;
        anyhow::ensure!(
            std::fs::metadata(path)?.len() == visible.len(),
            "transcript bus grew outside the shared writer during tail copy"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
        }
        let replacement = super::transcript_bus::open_bus_append_file(&staged)?;
        std::fs::rename(&staged, path)?;
        *writer = replacement;
        report.bytes_before = visible.len();
        report.bytes_after += tail_len;
        report.applied = true;
        Ok(locked_at.elapsed().as_millis() as u64)
    })();
    if result.is_err() {
        std::fs::remove_file(&staged).ok();
    }
    let locked_ms = result?;
    report.locked_ms = locked_ms;
    if report.applied {
        tracing::info!(
            rows_read = report.rows_read,
            rows_kept = report.rows_kept,
            evidence_dropped = report.evidence_rows_dropped,
            bytes_before = report.bytes_before,
            bytes_after = report.bytes_after,
            trigger,
            locked_ms,
            "bus_compaction"
        );
        Ok(Some(report))
    } else {
        Ok(None)
    }
}

fn stage_bus(
    path: &Path,
    retention_days: u32,
    bytes_before: u64,
    owned: bool,
) -> Result<(CompactionReport, PathBuf)> {
    let cutoff = cutoff_for(retention_days);

    let staged = if owned {
        path.with_extension(format!("jsonl.compacting.owned-{}", std::process::id()))
    } else {
        path.with_extension("jsonl.compacting")
    };
    let result = (|| -> Result<CompactionReport> {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- staged path is derived from the bus path by `with_extension`, not from input.
        let mut out = std::fs::File::create(&staged)
            .with_context(|| format!("create staged bus {}", staged.display()))?;
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- same local bus path already opened for status above.
        let source = std::fs::File::open(path)
            .with_context(|| format!("open transcript bus {}", path.display()))?;

        let mut report = CompactionReport {
            rows_read: 0,
            rows_kept: 0,
            evidence_rows_dropped: 0,
            bytes_before,
            bytes_after: 0,
            applied: false,
            locked_ms: 0,
            locked_rows_read: 0,
            locked_bytes_read: 0,
        };
        let mut last_document: HashMap<String, String> = HashMap::new();
        let mut history_seen: HashSet<(String, u64)> = HashSet::new();
        for line in BufReader::new(source.take(bytes_before)).lines() {
            let line = line.with_context(|| format!("read transcript bus {}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            report.rows_read += 1;
            let parsed = serde_json::from_str::<serde_json::Value>(&line).ok();
            let schema = parsed
                .as_ref()
                .and_then(|row| row.get("schema"))
                .and_then(|value| value.as_str());
            let time = parsed.as_ref().and_then(|row| {
                ["emitted_at", "ts", "timestamp", "created_at"]
                    .iter()
                    .find_map(|key| row.get(*key).and_then(|value| value.as_str()))
            });
            let session_id = parsed
                .as_ref()
                .and_then(|row| row.get("session_id"))
                .and_then(|value| value.as_str());
            if schema == Some(EVIDENCE_SCHEMA)
                && let Some(row) = parsed.as_ref()
                && let (Some(session), Some(document)) = (
                    session_id,
                    row.get("rendered_text").and_then(|value| value.as_str()),
                )
            {
                last_document.insert(session.to_string(), document.to_string());
            }
            // Only a row this code positively identifies as aged-out evidence is
            // dropped. An unknown schema, or evidence with no readable timestamp,
            // is kept: not understanding a row is not grounds for deleting it.
            let keep_terminal_revision = parsed.as_ref().is_some_and(|row| {
                row.get("reducer_action").and_then(|v| v.as_str()) == Some("apply_manual_edit")
                    && row.get("terminal").and_then(|v| v.as_bool()) == Some(true)
            });
            let drop = schema == Some(EVIDENCE_SCHEMA)
                && !keep_terminal_revision
                && time.is_some_and(|t| t < cutoff.as_str());
            let history = (schema == Some(EVIDENCE_SCHEMA))
                .then(|| super::transcript_bus::compact_history_row(&line))
                .flatten();
            if let Some((session, revision, _)) = history.as_ref() {
                if !drop {
                    history_seen.insert((session.clone(), *revision));
                }
            } else if schema == Some(super::transcript_bus::HISTORY_SCHEMA)
                && let (Some(session), Some(revision)) = (
                    session_id,
                    parsed
                        .as_ref()
                        .and_then(|row| row.get("revision"))
                        .and_then(|value| value.as_u64()),
                )
            {
                history_seen.insert((session.to_string(), revision));
            }
            if drop {
                report.evidence_rows_dropped += 1;
                if let Some((session, revision, compact)) = history
                    && history_seen.insert((session, revision))
                {
                    report.rows_kept += 1;
                    report.bytes_after += compact.len() as u64 + 1;
                    writeln!(out, "{compact}")?;
                }
                continue;
            }
            let retained = if schema == Some(DELIVERY_SCHEMA)
                && let Some(row) = parsed.as_ref()
                && row.get("status").and_then(|value| value.as_str()) == Some("session_ended")
                && row.get("rendered_text").is_none()
                && let Some(document) = session_id.and_then(|id| last_document.get(id))
            {
                let mut row = row.clone();
                row["rendered_text"] = serde_json::Value::String(document.clone());
                serde_json::to_string(&row)?
            } else {
                line
            };
            report.rows_kept += 1;
            report.bytes_after += retained.len() as u64 + 1;
            writeln!(out, "{retained}")?;
        }
        out.flush()?;
        drop(out);
        Ok(report)
    })();
    if result.is_err() {
        std::fs::remove_file(&staged).ok();
    }
    result.map(|report| (report, staged))
}

fn compact_bus_inner(
    path: &Path,
    retention_days: u32,
    dry_run: bool,
    owned: bool,
    mut writer: Option<&mut std::fs::File>,
) -> Result<CompactionReport> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("stat transcript bus {}", path.display()))?;
    let bytes_before = metadata.len();
    let modified_before = metadata.modified().ok();

    if let Some(modified) = modified_before {
        let quiet_for = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        anyhow::ensure!(
            dry_run || owned || quiet_for >= QUIET_PERIOD,
            "transcript bus was written {}s ago; a session may be open. \
             Stop Codescribe (or wait {}s) before compacting, or the rewrite \
             would discard rows appended meanwhile.",
            quiet_for.as_secs(),
            QUIET_PERIOD.saturating_sub(quiet_for).as_secs()
        );
    }

    let (mut report, staged) = stage_bus(path, retention_days, bytes_before, owned)?;
    if owned && report.evidence_rows_dropped == 0 {
        std::fs::remove_file(&staged).ok();
        report.bytes_after = report.bytes_before;
        return Ok(report);
    }

    if dry_run {
        std::fs::remove_file(&staged).ok();
        return Ok(report);
    }

    // The file may have grown while we were reading it. Our staged copy does
    // not contain those rows, so renaming over the original would delete them.
    let metadata_now = std::fs::metadata(path)
        .with_context(|| format!("re-stat transcript bus {}", path.display()))?;
    let unchanged =
        metadata_now.len() == bytes_before && metadata_now.modified().ok() == modified_before;
    if !unchanged {
        std::fs::remove_file(&staged).ok();
        anyhow::bail!(
            "transcript bus changed during compaction ({} -> {} bytes); \
             staged copy discarded and the original left untouched",
            bytes_before,
            metadata_now.len()
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
    }
    // Open before rename: if opening fails, the original file and descriptor
    // still point to the same visible inode.
    let replacement = if owned {
        Some(super::transcript_bus::open_bus_append_file(&staged)?)
    } else {
        None
    };
    std::fs::rename(&staged, path)
        .with_context(|| format!("replace transcript bus {}", path.display()))?;
    if let (Some(writer), Some(replacement)) = (writer.as_mut(), replacement) {
        **writer = replacement;
    }
    report.applied = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_append_survives_compaction_and_remains_on_visible_path() {
        use crate::presentation::transcript_bus::{
            TranscriptBus, TranscriptMode, TranscriptSession,
        };

        let dir = temp("live-append");
        let path = dir.join("transcript-events.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "live-append".into(),
                mode: TranscriptMode::Dictation,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            path.clone(),
            None,
        )
        .expect("open bus");
        bus.publish_started();
        let before = std::fs::read_to_string(&path).expect("read start");
        append_aged_evidence(&path);
        // Keep the fixture small while exercising the production lock path;
        // the threshold check is separately tested with an ordinary tiny file.
        let shared = super::super::transcript_bus::shared_bus_file(&path).unwrap();
        let mut writer = shared.lock().unwrap();
        let report = compact_bus_inner(&path, 14, false, true, Some(&mut writer))
            .expect("app can compact its live bus");
        drop(writer);
        assert!(report.applied);
        assert_eq!(report.evidence_rows_dropped, 1);
        bus.publish_ended(
            crate::presentation::transcript_bus::TranscriptSessionEndReason::Completed,
            false,
            crate::presentation::transcript_bus::TranscriptDelivery::Unattempted,
        );
        let after = std::fs::read_to_string(&path).expect("read after compact");
        assert!(after.contains(&before));
        assert!(after.contains("session_ended"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn app_compaction_preserves_every_concurrent_session_append() {
        use crate::presentation::transcript_bus::{
            TranscriptBus, TranscriptMode, TranscriptSession,
        };
        let dir = temp("concurrent-append");
        let path = dir.join("transcript-events.jsonl");
        let first = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "seed".into(),
                mode: TranscriptMode::Dictation,
                has_latched_target: false,
                latched_target_is_self: false,
            },
            path.clone(),
            None,
        )
        .unwrap();
        first.publish_started();
        append_aged_evidence(&path);
        let shared = super::super::transcript_bus::shared_bus_file(&path).unwrap();
        let mut writer = shared.lock().unwrap();
        let writer_path = path.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            for index in 0..100 {
                let bus = TranscriptBus::open_at(
                    TranscriptSession {
                        session_id: format!("concurrent-{index}"),
                        mode: TranscriptMode::Dictation,
                        has_latched_target: false,
                        latched_target_is_self: false,
                    },
                    writer_path.clone(),
                    None,
                )
                .unwrap();
                bus.publish_started();
            }
        });
        ready_rx.recv().unwrap();
        let report = compact_bus_inner(&path, 14, false, true, Some(&mut writer)).unwrap();
        assert!(report.applied);
        drop(writer);
        handle.join().unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        for index in 0..100 {
            assert!(
                contents.contains(&format!("concurrent-{index}")),
                "lost row {index}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn owned_compaction_locked_reads_depend_on_appended_tail_not_prefix() {
        use crate::presentation::transcript_bus::{
            TranscriptBus, TranscriptMode, TranscriptSession,
        };
        const TAIL_ROWS: usize = 10;
        let mut locked_counts = Vec::new();
        for prefix_bytes in [1024 * 1024, 10 * 1024 * 1024] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("transcript-events.jsonl");
            let buses: Vec<_> = (0..TAIL_ROWS)
                .map(|index| {
                    TranscriptBus::open_at(
                        TranscriptSession {
                            session_id: format!("new-take-{index}"),
                            mode: TranscriptMode::Dictation,
                            has_latched_target: false,
                            latched_target_is_self: false,
                        },
                        path.clone(),
                        None,
                    )
                    .unwrap()
                })
                .collect();
            let row = format!(
                "{{\"schema\":\"codescribe.transcript-evidence.v1\",\"emitted_at\":\"2020-01-01T00:00:00Z\",\"padding\":\"{}\"}}\n",
                "x".repeat(2048)
            );
            let mut out = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            while out.metadata().unwrap().len() < prefix_bytes {
                out.write_all(row.as_bytes()).unwrap();
            }
            drop(out);
            let recorded_len = std::fs::metadata(&path).unwrap().len();
            let report = compact_bus_owned_once(&path, 14, "test", 1, || {
                std::thread::scope(|scope| {
                    scope.spawn(|| {
                        for bus in &buses {
                            bus.publish_started();
                        }
                    }).join().unwrap();
                });
            })
            .unwrap()
            .unwrap();
            assert!(report.applied);
            assert_eq!(report.locked_rows_read, TAIL_ROWS as u64);
            assert_eq!(
                report.locked_bytes_read,
                report.bytes_before - recorded_len,
                "the locked reader copied exactly the appended tail"
            );
            let final_bus = std::fs::read_to_string(&path).unwrap();
            for index in 0..TAIL_ROWS {
                assert!(final_bus.contains(&format!("new-take-{index}")));
            }
            locked_counts.push(report.locked_rows_read);
        }
        assert_eq!(locked_counts, [TAIL_ROWS as u64; 2]);
    }

    #[test]
    fn owned_compaction_refuses_a_replaced_bus_before_rename() {
        let dir = temp("replaced-during-compaction");
        let path = dir.join("transcript-events.jsonl");
        let row = b"{\"schema\":\"codescribe.transcript-evidence.v1\",\"emitted_at\":\"2020-01-01T00:00:00Z\"}\n";
        let mut out = std::fs::File::create(&path).unwrap();
        out.write_all(b"{\"schema\":\"codescribe.transcript.v1\",\"session_id\":\"seed\",\"status\":\"session_started\"}\n").unwrap();
        while out.metadata().unwrap().len() < COMPACTION_THRESHOLD_BYTES {
            out.write_all(row).unwrap();
        }
        drop(out);
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || compact_bus_owned(&worker_path, 14, "test"));
        let staged = path.with_extension(format!("jsonl.compacting.owned-{}", std::process::id()));
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::fs::metadata(&staged).map(|m| m.len()).unwrap_or(0) == 0
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            std::fs::metadata(&staged).unwrap().len() > 0,
            "compaction never entered the off-lock stage"
        );
        std::fs::rename(&path, dir.join("original.jsonl")).unwrap();
        std::fs::write(&path, b"replacement\n").unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(
            error.to_string().contains("shrank or was replaced"),
            "{error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement\n");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compaction_retains_a_takes_projected_document() {
        use crate::presentation::transcript_projection::TranscriptProjectionReader;
        let dir = temp("document");
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.transcript.v1","sequence":1,"session_id":"take-1","status":"session_started","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript-evidence.v1","sequence":2,"session_id":"take-1","reducer_revision":7,"reducer_action":"apply_ledger_decision","occurrence_session_id":"take-1","capture_epoch":1,"sample_start":0,"sample_end":16000,"document_index":0,"rendered_text":"Five Iwo","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript.v1","sequence":3,"session_id":"take-1","status":"session_ended","terminal":true,"emitted_at":"2020-01-01T00:00:00Z"}"#,
            ],
        );
        let document = |path: &Path| {
            let mut reader = TranscriptProjectionReader::new();
            reader
                .push_bytes(&std::fs::read(path).unwrap())
                .into_iter()
                .map(Result::unwrap)
                .last()
                .unwrap()
                .rendered_text
        };
        let before = document(&path);
        let shared = super::super::transcript_bus::shared_bus_file(&path).unwrap();
        let mut writer = shared.lock().unwrap();
        let report = compact_bus_inner(&path, 14, false, true, Some(&mut writer)).unwrap();
        drop(writer);
        assert_eq!(report.evidence_rows_dropped, 1);
        assert_eq!(before, "Five Iwo");
        assert_eq!(document(&path), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn app_skips_a_bus_below_the_compaction_threshold() {
        let dir = temp("below-threshold");
        let path = write_bus(&dir, &[r#"{"schema":"codescribe.transcript.v1"}"#]);
        let before = std::fs::read(&path).unwrap();
        assert!(compact_bus_owned(&path, 14, "idle").unwrap().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn app_does_not_rewrite_when_no_evidence_has_expired() {
        let dir = temp("fresh-no-rewrite");
        let fresh = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let path = write_bus(
            &dir,
            &[&format!(
                r#"{{"schema":"codescribe.transcript-evidence.v1","emitted_at":"{fresh}"}}"#
            )],
        );
        let before = std::fs::read(&path).unwrap();
        let shared = super::super::transcript_bus::shared_bus_file(&path).unwrap();
        let mut writer = shared.lock().unwrap();
        let report = compact_bus_inner(&path, 14, false, true, Some(&mut writer)).unwrap();
        drop(writer);
        assert!(!report.applied);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn write_bus(dir: &Path, rows: &[&str]) -> PathBuf {
        let path = dir.join("transcript-events.jsonl");
        std::fs::write(&path, format!("{}\n", rows.join("\n"))).expect("write bus");
        path
    }

    fn append_aged_evidence(path: &Path) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{\"schema\":\"codescribe.transcript-evidence.v1\",\"emitted_at\":\"2020-01-01T00:00:00Z\"}\n")
            .unwrap();
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codescribe-bus-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn status_separates_the_two_roles_sharing_the_file() {
        let dir = temp("status");
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.transcript.v1","emitted_at":"2026-09-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript-evidence.v1","emitted_at":"2026-09-02T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript-evidence.v1","emitted_at":"2026-09-03T00:00:00Z"}"#,
            ],
        );
        let status = bus_status(&path).expect("status");
        assert_eq!(status.delivery_rows, 1);
        assert_eq!(status.evidence_rows, 2);
        assert_eq!(status.first_seen.as_deref(), Some("2026-09-01T00:00:00Z"));
        assert_eq!(status.last_seen.as_deref(), Some("2026-09-03T00:00:00Z"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_previews_what_each_window_would_reclaim() {
        let dir = temp("preview");
        let fresh = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.transcript-evidence.v1","emitted_at":"2020-01-01T00:00:00Z"}"#,
                &format!(
                    r#"{{"schema":"codescribe.transcript-evidence.v1","emitted_at":"{fresh}"}}"#
                ),
            ],
        );
        let status = bus_status(&path).expect("status");
        assert_eq!(status.retention_preview.len(), 4);
        for (days, bytes) in &status.retention_preview {
            assert!(
                *bytes > 0,
                "the 2020 row is outside every window, including {days}d"
            );
            assert!(
                *bytes < status.evidence_bytes,
                "the fresh row is inside every window, so no window reclaims all of it"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compaction_keeps_every_delivery_row_however_old() {
        let dir = temp("delivery");
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.transcript.v1","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript-evidence.v1","emitted_at":"2020-01-01T00:00:00Z"}"#,
            ],
        );
        let report = compact_bus(&path, 14, true).expect("dry run");
        assert_eq!(report.rows_read, 2);
        assert_eq!(report.rows_kept, 1, "the ancient delivery row survives");
        assert_eq!(report.evidence_rows_dropped, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn applied_compaction_keeps_all_delivery_and_fresh_evidence() {
        let dir = temp("applied-retention");
        let fresh = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.transcript.v1","session_id":"first","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript-evidence.v1","session_id":"first","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"{"schema":"codescribe.transcript.v1","session_id":"second","emitted_at":"2020-01-02T00:00:00Z"}"#,
                &format!(
                    r#"{{"schema":"codescribe.transcript-evidence.v1","session_id":"fresh","emitted_at":"{fresh}"}}"#
                ),
            ],
        );
        let shared = super::super::transcript_bus::shared_bus_file(&path).unwrap();
        let mut writer = shared.lock().unwrap();
        let report = compact_bus_inner(&path, 14, false, true, Some(&mut writer)).unwrap();
        drop(writer);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(report.rows_kept, 3);
        assert_eq!(report.evidence_rows_dropped, 1);
        assert!(contents.contains("\"session_id\":\"first\""));
        assert!(contents.contains("\"session_id\":\"second\""));
        assert!(contents.contains("\"session_id\":\"fresh\""));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_schema_is_never_dropped() {
        let dir = temp("unknown");
        let path = write_bus(
            &dir,
            &[
                r#"{"schema":"codescribe.something.v9","emitted_at":"2020-01-01T00:00:00Z"}"#,
                r#"not json at all"#,
                r#"{"schema":"codescribe.transcript-evidence.v1"}"#,
            ],
        );
        let report = compact_bus(&path, 14, true).expect("dry run");
        assert_eq!(
            report.rows_kept, 3,
            "unknown schema, unparseable row and timestamp-less evidence all stay"
        );
        assert_eq!(report.evidence_rows_dropped, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_evidence_survives_the_cutoff() {
        let dir = temp("recent");
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let path = write_bus(
            &dir,
            &[&format!(
                r#"{{"schema":"codescribe.transcript-evidence.v1","emitted_at":"{now}"}}"#
            )],
        );
        let report = compact_bus(&path, 14, true).expect("dry run");
        assert_eq!(report.evidence_rows_dropped, 0);
        assert_eq!(report.rows_kept, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_live_bus_refuses_compaction() {
        let dir = temp("live");
        let path = write_bus(
            &dir,
            &[r#"{"schema":"codescribe.transcript.v1","emitted_at":"2026-09-01T00:00:00Z"}"#],
        );
        // The file was just written, so it is inside the quiet period.
        let error = compact_bus(&path, 14, false).expect_err("must refuse a live bus");
        assert!(
            error.to_string().contains("a session may be open"),
            "{error}"
        );
        assert!(
            compact_bus(&path, 14, true).is_ok(),
            "a dry run is read-only and stays allowed"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_dry_run_leaves_no_staged_file_behind() {
        let dir = temp("staged");
        let path = write_bus(
            &dir,
            &[r#"{"schema":"codescribe.transcript.v1","emitted_at":"2026-09-01T00:00:00Z"}"#],
        );
        compact_bus(&path, 14, true).expect("dry run");
        let staged: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("compacting"))
            .collect();
        assert!(staged.is_empty(), "staged copy must be cleaned up");
        std::fs::remove_dir_all(&dir).ok();
    }
}
