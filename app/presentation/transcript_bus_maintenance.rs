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
//! SAFETY. The app holds an append descriptor on this file while it runs.
//! Rewriting through a rename swaps the inode, and a writer holding the old
//! descriptor would keep appending into an unlinked file, losing every row it
//! wrote during the rewrite. Compaction therefore refuses to start when the bus
//! was written recently, and re-checks length and mtime before the rename,
//! discarding its own work rather than overwriting an append it did not see.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// Size past which the bus is worth compacting. Below this the reclaim is not
/// worth the risk of rewriting a file another process may be appending to.
pub const COMPACTION_THRESHOLD_BYTES: u64 = 128 * 1024 * 1024;

/// Default evidence retention. Deliberately generous: the cost of keeping
/// evidence a few days too long is disk, and the cost of dropping it a day too
/// early is a diagnosis that can no longer be made. `status` prints what
/// shorter windows would reclaim so the operator can choose a sharper one.
pub const DEFAULT_EVIDENCE_RETENTION_DAYS: u32 = 14;

/// Windows offered in the status preview, in days.
const PREVIEW_WINDOWS: [u32; 4] = [3, 7, 14, 30];

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
fn row_schema_and_time(line: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return (None, None);
    };
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let time = ["emitted_at", "ts", "timestamp", "created_at"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string);
    (schema, time)
}

/// Summarise the bus: size, composition and span.
pub fn bus_status(path: &Path) -> Result<BusStatus> {
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
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
    let cutoffs: Vec<(u32, String)> = PREVIEW_WINDOWS
        .iter()
        .map(|days| (*days, cutoff_for(*days)))
        .collect();
    let mut reclaimable = vec![0u64; cutoffs.len()];
    for line in BufReader::new(file).lines() {
        let line = line.with_context(|| format!("read bus {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let width = line.len() as u64 + 1;
        let (schema, time) = row_schema_and_time(&line);
        match schema.as_deref() {
            Some(DELIVERY_SCHEMA) => {
                status.delivery_rows += 1;
                status.delivery_bytes += width;
            }
            Some(EVIDENCE_SCHEMA) => {
                status.evidence_rows += 1;
                status.evidence_bytes += width;
                if let Some(time) = time.as_deref() {
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
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("stat transcript bus {}", path.display()))?;
    let bytes_before = metadata.len();
    let modified_before = metadata.modified().ok();

    if let Some(modified) = modified_before {
        let quiet_for = SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO);
        anyhow::ensure!(
            dry_run || quiet_for >= QUIET_PERIOD,
            "transcript bus was written {}s ago; a session may be open. \
             Stop Codescribe (or wait {}s) before compacting, or the rewrite \
             would discard rows appended meanwhile.",
            quiet_for.as_secs(),
            QUIET_PERIOD.saturating_sub(quiet_for).as_secs()
        );
    }

    let cutoff = cutoff_for(retention_days);

    let staged = path.with_extension("jsonl.compacting");
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
    };
    for line in BufReader::new(source).lines() {
        let line = line.with_context(|| format!("read transcript bus {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        report.rows_read += 1;
        let (schema, time) = row_schema_and_time(&line);
        // Only a row this code positively identifies as aged-out evidence is
        // dropped. An unknown schema, or evidence with no readable timestamp,
        // is kept: not understanding a row is not grounds for deleting it.
        let drop = schema.as_deref() == Some(EVIDENCE_SCHEMA)
            && time.as_deref().is_some_and(|t| t < cutoff.as_str());
        if drop {
            report.evidence_rows_dropped += 1;
            continue;
        }
        report.rows_kept += 1;
        report.bytes_after += line.len() as u64 + 1;
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    drop(out);

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
    std::fs::rename(&staged, path)
        .with_context(|| format!("replace transcript bus {}", path.display()))?;
    report.applied = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bus(dir: &Path, rows: &[&str]) -> PathBuf {
        let path = dir.join("transcript-events.jsonl");
        std::fs::write(&path, format!("{}\n", rows.join("\n"))).expect("write bus");
        path
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
