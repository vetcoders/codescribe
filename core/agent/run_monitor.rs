//! Resident run-monitor harness: durable registration, bounded observation,
//! and heartbeat decisions for external Vibecrafted runs.
//!
//! The interactive agent turn only *registers* a monitor; the application
//! runtime owns the polling loop (`app::agent::monitor`). This module holds
//! everything that must survive an app restart: the monitor record with its
//! canonical `run_id`, cursors, and notification ledger, plus the pure state
//! machine `started → running → stale/blocked → completed/failed/needs-rerun`.
//!
//! Observation reads the Vibecrafted control-plane `meta.json` as the primary
//! cursor surface; raw transcript logs are fallback evidence only, clipped to
//! [`MAX_HEARTBEAT_EXCERPT_BYTES`] so a heartbeat can never flood a thread.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::util::safe_path::{safe_canonicalize, safe_read_to_string};

/// Default seconds between polls of one monitored run.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 15;
/// Default seconds of meta.json silence before a live run is declared stale.
pub const DEFAULT_STALE_AFTER_SECS: u64 = 300;
/// Hard cap on transcript/report excerpt bytes embedded in a heartbeat.
pub const MAX_HEARTBEAT_EXCERPT_BYTES: usize = 2048;
/// Consecutive meta read failures before the monitor reports `Blocked`.
pub const MAX_READ_FAILURES_BEFORE_BLOCKED: u32 = 5;
/// Backoff ceiling for failing polls.
pub const MAX_BACKOFF_SECS: u64 = 300;

const MONITOR_FILE_NAME: &str = "monitors.json";

/// Monitor-visible lifecycle of one observed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunMonitorState {
    /// Registered and live, but no cursor movement observed yet.
    Started,
    /// Cursor movement seen since the last poll — the run is alive.
    Running,
    /// No progress past `stale_after_secs` and no promised artifact to explain it.
    Stale,
    /// Meta reports a block, or it stayed unreadable past the failure budget.
    Blocked,
    /// Terminal success with the promised report artifact present.
    Completed,
    /// Terminal failure: meta says `failed`, or the exit code is non-zero.
    Failed,
    /// Terminal success *claimed* without its promised artifact — never
    /// reported as [`Completed`](Self::Completed).
    NeedsRerun,
}

impl RunMonitorState {
    /// Stable kebab-case wire/log name for this state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Running => "running",
            Self::Stale => "stale",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::NeedsRerun => "needs-rerun",
        }
    }

    /// Terminal states end the monitor after exactly one notification.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::NeedsRerun)
    }
}

/// Durable record of one registered monitor. `monitor_id` is the harness-side
/// identity; `run_id` stays the canonical Vibecrafted identity — never merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMonitorRecord {
    pub monitor_id: String,
    pub run_id: String,
    /// Durable thread the heartbeats re-enter (existing-thread continuity).
    pub thread_store_id: String,
    pub meta_path: PathBuf,
    #[serde(default)]
    pub transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub report_path: Option<PathBuf>,
    pub state: RunMonitorState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Last state a heartbeat was delivered for — the no-duplicate ledger.
    #[serde(default)]
    pub last_notified_state: Option<RunMonitorState>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub poll_interval_secs: u64,
    pub stale_after_secs: u64,
    #[serde(default)]
    pub consecutive_read_failures: u32,
    pub next_poll_at: DateTime<Utc>,
    /// Progress cursors: byte lengths observed at the last successful poll.
    #[serde(default)]
    pub meta_len: u64,
    #[serde(default)]
    pub transcript_len: u64,
}

impl RunMonitorRecord {
    /// Still worth polling: neither finished nor cancelled.
    pub fn is_active(&self) -> bool {
        !self.done && !self.cancelled
    }
}

/// Parameters for registering a monitor; everything else is derived.
#[derive(Debug, Clone)]
pub struct RunMonitorRegistration {
    pub run_id: String,
    pub thread_store_id: String,
    pub meta_path: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub report_path: Option<PathBuf>,
    pub poll_interval_secs: u64,
    pub stale_after_secs: u64,
}

/// On-disk shape of the monitor store: a flat list, rewritten atomically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RunMonitorStoreData {
    monitors: Vec<RunMonitorRecord>,
}

/// Durable JSON store for monitor records. Same placement contract as
/// [`super::ThreadStore`]: `CODESCRIBE_DATA_DIR` override, atomic tmp+rename
/// writes, `new_in` for isolated tests.
#[derive(Debug, Clone)]
pub struct RunMonitorStore {
    file_path: PathBuf,
}

impl RunMonitorStore {
    /// Open the store under the app data dir (`CODESCRIBE_DATA_DIR`-aware).
    pub fn new() -> Result<Self> {
        Self::new_in(super::thread_store::app_data_dir().join("agent_monitors"))
    }

    /// Open the store in an explicit directory, creating it if absent. The
    /// isolation seam tests use instead of the shared data dir.
    pub fn new_in<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create monitor dir: {}", dir.display()))?;
        Ok(Self {
            file_path: dir.join(MONITOR_FILE_NAME),
        })
    }

    /// Path of the backing JSON file — the inspectable evidence surface.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Read the store. A missing file is an empty store, not an error.
    fn load(&self) -> Result<RunMonitorStoreData> {
        if !self.file_path.exists() {
            return Ok(RunMonitorStoreData::default());
        }
        let raw = fs::read_to_string(&self.file_path)
            .with_context(|| format!("Failed to read {}", self.file_path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse {}", self.file_path.display()))
    }

    /// Write the store atomically (tmp file + rename) so a crash mid-write
    /// cannot leave a truncated ledger behind.
    fn save(&self, data: &RunMonitorStoreData) -> Result<()> {
        let raw = serde_json::to_string_pretty(data).context("Failed to serialize monitors")?;
        let tmp = self.file_path.with_extension("tmp");
        fs::write(&tmp, raw)
            .with_context(|| format!("Failed to write temporary {}", tmp.display()))?;
        fs::rename(&tmp, &self.file_path)
            .with_context(|| format!("Failed to store {}", self.file_path.display()))?;
        Ok(())
    }

    /// Register a monitor. Idempotent per `(run_id, thread_store_id)`: an
    /// already-active monitor for the same pair is returned unchanged so a
    /// repeated tool call cannot spawn duplicate notifiers.
    pub fn register(
        &self,
        registration: RunMonitorRegistration,
        now: DateTime<Utc>,
    ) -> Result<RunMonitorRecord> {
        let mut data = self.load()?;
        if let Some(existing) = data.monitors.iter().find(|record| {
            record.is_active()
                && record.run_id == registration.run_id
                && record.thread_store_id == registration.thread_store_id
        }) {
            return Ok(existing.clone());
        }

        let record = RunMonitorRecord {
            monitor_id: format!("mon-{}", uuid::Uuid::new_v4()),
            run_id: registration.run_id,
            thread_store_id: registration.thread_store_id,
            meta_path: registration.meta_path,
            transcript_path: registration.transcript_path,
            report_path: registration.report_path,
            state: RunMonitorState::Started,
            created_at: now,
            updated_at: now,
            last_notified_state: None,
            done: false,
            cancelled: false,
            poll_interval_secs: registration.poll_interval_secs.max(1),
            stale_after_secs: registration.stale_after_secs.max(1),
            consecutive_read_failures: 0,
            next_poll_at: now,
            meta_len: 0,
            transcript_len: 0,
        };
        data.monitors.push(record.clone());
        self.save(&data)?;
        Ok(record)
    }

    /// Every record, including finished and cancelled ones.
    pub fn list(&self) -> Result<Vec<RunMonitorRecord>> {
        Ok(self.load()?.monitors)
    }

    /// Only the records still worth polling.
    pub fn active(&self) -> Result<Vec<RunMonitorRecord>> {
        Ok(self
            .load()?
            .monitors
            .into_iter()
            .filter(RunMonitorRecord::is_active)
            .collect())
    }

    /// Look one record up by its harness-side `monitor_id`.
    pub fn get(&self, monitor_id: &str) -> Result<Option<RunMonitorRecord>> {
        Ok(self
            .load()?
            .monitors
            .into_iter()
            .find(|record| record.monitor_id == monitor_id))
    }

    /// Persist an updated record (checkpoint). Unknown ids are an error — a
    /// checkpoint for a vanished monitor means someone truncated the store.
    pub fn update(&self, record: &RunMonitorRecord) -> Result<()> {
        let mut data = self.load()?;
        let slot = data
            .monitors
            .iter_mut()
            .find(|candidate| candidate.monitor_id == record.monitor_id)
            .with_context(|| format!("Monitor {} is not registered", record.monitor_id))?;
        *slot = record.clone();
        self.save(&data)
    }

    /// Cancel a monitor: it stops polling and never notifies again. The record
    /// stays in the store as inspectable evidence.
    pub fn cancel(&self, monitor_id: &str, now: DateTime<Utc>) -> Result<bool> {
        let mut data = self.load()?;
        let Some(slot) = data
            .monitors
            .iter_mut()
            .find(|candidate| candidate.monitor_id == monitor_id)
        else {
            return Ok(false);
        };
        slot.cancelled = true;
        slot.done = true;
        slot.updated_at = now;
        self.save(&data)?;
        Ok(true)
    }

    /// Remove finished records, keeping active ones. Returns removed count.
    pub fn prune_done(&self) -> Result<usize> {
        let mut data = self.load()?;
        let before = data.monitors.len();
        data.monitors.retain(RunMonitorRecord::is_active);
        let removed = before - data.monitors.len();
        if removed > 0 {
            self.save(&data)?;
        }
        Ok(removed)
    }
}

/// Fields extracted from a Vibecrafted control-plane `meta.json`. Unknown
/// fields are ignored — the control plane owns its own schema evolution.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunMetaSnapshot {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub report: Option<String>,
    #[serde(default)]
    pub transcript: Option<String>,
}

/// One successful observation of a monitored run.
#[derive(Debug, Clone)]
pub struct RunObservation {
    pub state: RunMonitorState,
    pub exit_code: Option<i64>,
    pub meta_status: Option<String>,
    pub report_present: bool,
    pub report_path: Option<PathBuf>,
    pub meta_len: u64,
    pub transcript_len: u64,
}

/// Pure classifier for the monitor state machine.
///
/// Encodes the vc-workflow 3-signal doctrine: a terminal meta *or* an
/// agreeing pair (promised report present + no further progress) is enough to
/// act; a success claim without its promised artifact is `NeedsRerun`, never a
/// silent `Completed`.
pub fn classify_run_state(
    meta_status: Option<&str>,
    exit_code: Option<i64>,
    completed_at_present: bool,
    report_present: bool,
    age_secs: i64,
    stale_after_secs: u64,
    progressed: bool,
) -> RunMonitorState {
    let terminal_meta = matches!(meta_status, Some("completed" | "failed"))
        || completed_at_present
        || exit_code.is_some();
    if terminal_meta {
        let failed = matches!(meta_status, Some("failed")) || exit_code.unwrap_or(0) != 0;
        if failed {
            return RunMonitorState::Failed;
        }
        return if report_present {
            RunMonitorState::Completed
        } else {
            RunMonitorState::NeedsRerun
        };
    }
    if matches!(meta_status, Some("blocked")) {
        return RunMonitorState::Blocked;
    }
    // Observed progress (cursor movement) beats mtime age: a run that just
    // wrote is alive regardless of how long the previous silence lasted.
    if progressed {
        return RunMonitorState::Running;
    }
    if age_secs >= stale_after_secs as i64 {
        // Known control-plane skew: meta can stay `active`/`stalled` after a
        // real completion. Promised report present + no meta progress are two
        // agreeing signals — surface completion instead of a false stall.
        if report_present {
            return RunMonitorState::Completed;
        }
        return RunMonitorState::Stale;
    }
    RunMonitorState::Started
}

/// Observe one record against the filesystem. Errors mean the meta surface is
/// unreadable — the caller owns the failure counter and `Blocked` escalation.
pub fn observe_record(record: &RunMonitorRecord, now: DateTime<Utc>) -> Result<RunObservation> {
    // Canonicalize + capability-open the control-plane meta (defense in depth
    // against a durable record whose path was rewritten off-root).
    let meta_path = safe_canonicalize(&record.meta_path)
        .with_context(|| format!("Failed to canonicalize {}", record.meta_path.display()))?;
    let metadata = fs::metadata(&meta_path)
        .with_context(|| format!("Failed to stat {}", meta_path.display()))?;
    let meta_len = metadata.len();
    let age_secs = metadata
        .modified()
        .ok()
        .map(|modified| {
            let modified: DateTime<Utc> = modified.into();
            (now - modified).num_seconds()
        })
        .unwrap_or(0);

    let raw = safe_read_to_string(&meta_path)
        .with_context(|| format!("Failed to read {}", meta_path.display()))?;
    let snapshot: RunMetaSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", meta_path.display()))?;

    let report_path = record
        .report_path
        .clone()
        .or_else(|| snapshot.report.as_deref().map(PathBuf::from))
        .and_then(|path| safe_canonicalize(&path).ok());
    let report_present = report_path.as_deref().is_some_and(|path| path.is_file());

    let transcript_len = record
        .transcript_path
        .clone()
        .or_else(|| snapshot.transcript.as_deref().map(PathBuf::from))
        .and_then(|path| safe_canonicalize(&path).ok())
        .and_then(|path| fs::metadata(path).ok())
        .map(|meta| meta.len())
        .unwrap_or(0);

    let progressed = record.meta_len == 0
        || meta_len != record.meta_len
        || transcript_len != record.transcript_len;

    let state = classify_run_state(
        snapshot.status.as_deref(),
        snapshot.exit_code,
        snapshot.completed_at.is_some(),
        report_present,
        age_secs,
        record.stale_after_secs,
        progressed,
    );

    Ok(RunObservation {
        state,
        exit_code: snapshot.exit_code,
        meta_status: snapshot.status,
        report_present,
        report_path,
        meta_len,
        transcript_len,
    })
}

/// Why a heartbeat is being delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatKind {
    /// A previously-notified stall or block started progressing again.
    Recovered,
    /// The run went quiet past its stale bar.
    Stalled,
    /// The run's control-plane meta reports a block or is unreadable.
    Blocked,
    /// The run finished successfully.
    Completed,
    /// The run finished unsuccessfully.
    Failed,
    /// The run claimed success without its promised report artifact.
    NeedsRerun,
}

/// Decide whether `new_state` warrants a heartbeat into the thread.
///
/// Guarantees: never after `done`/`cancelled`, never twice for the same state
/// (the `last_notified_state` ledger), and never for quiet forward progress —
/// only stall, block, recovery, and terminal outcomes re-enter the thread.
pub fn decide_heartbeat(
    record: &RunMonitorRecord,
    new_state: RunMonitorState,
) -> Option<HeartbeatKind> {
    if record.done || record.cancelled {
        return None;
    }
    if record.last_notified_state == Some(new_state) {
        return None;
    }
    match new_state {
        RunMonitorState::Started => None,
        RunMonitorState::Running => match record.last_notified_state {
            Some(RunMonitorState::Stale | RunMonitorState::Blocked) => {
                Some(HeartbeatKind::Recovered)
            }
            _ => None,
        },
        RunMonitorState::Stale => Some(HeartbeatKind::Stalled),
        RunMonitorState::Blocked => Some(HeartbeatKind::Blocked),
        RunMonitorState::Completed => Some(HeartbeatKind::Completed),
        RunMonitorState::Failed => Some(HeartbeatKind::Failed),
        RunMonitorState::NeedsRerun => Some(HeartbeatKind::NeedsRerun),
    }
}

/// Exponential backoff for failing polls: `interval * 2^failures`, capped.
pub fn next_poll_delay_secs(base_interval_secs: u64, consecutive_failures: u32) -> u64 {
    let shift = consecutive_failures.min(8);
    base_interval_secs
        .saturating_mul(1u64 << shift)
        .clamp(1, MAX_BACKOFF_SECS)
}

/// Read at most `max_bytes` from the end of a file, lossy-decoded. Returns
/// `None` when the file is absent or empty — heartbeats degrade gracefully.
pub fn bounded_tail(path: &Path, max_bytes: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }
    let start = len.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity(max_bytes.min(len as usize));
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Human heartbeat text delivered into the existing thread. Bounded by
/// construction: the only unbounded input is the excerpt, clipped upstream.
pub fn heartbeat_text(
    kind: HeartbeatKind,
    record: &RunMonitorRecord,
    observation: &RunObservation,
    excerpt: Option<&str>,
) -> String {
    let run_id = &record.run_id;
    let mut text = match kind {
        HeartbeatKind::Recovered => {
            format!("Run `{run_id}` recovered and is progressing again.")
        }
        HeartbeatKind::Stalled => format!(
            "Run `{run_id}` looks stalled: no control-plane progress for over {}s.",
            record.stale_after_secs
        ),
        HeartbeatKind::Blocked => format!(
            "Run `{run_id}` is blocked: its control-plane meta is unreadable or reports a block."
        ),
        HeartbeatKind::Completed => match observation.report_path.as_deref() {
            Some(report) => format!("Run `{run_id}` completed. Report: {}", report.display()),
            None => format!("Run `{run_id}` completed."),
        },
        HeartbeatKind::Failed => match observation.exit_code {
            Some(code) => format!("Run `{run_id}` failed (exit code {code})."),
            None => format!("Run `{run_id}` failed."),
        },
        HeartbeatKind::NeedsRerun => format!(
            "Run `{run_id}` claims success but its promised report artifact is missing — needs rerun."
        ),
    };
    if let Some(excerpt) = excerpt {
        text.push_str("\n\nLast output:\n```\n");
        text.push_str(excerpt);
        text.push_str("\n```");
    }
    text
}

/// Validate a caller-supplied run id before it ever touches a path join.
pub fn is_valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && !run_id.starts_with('.')
}

/// Distinct states currently represented among active monitors — the cheap
/// inspection surface for status UIs.
pub fn active_state_summary(records: &[RunMonitorRecord]) -> Vec<(RunMonitorState, usize)> {
    let mut seen: Vec<(RunMonitorState, usize)> = Vec::new();
    let mut order: HashSet<&'static str> = HashSet::new();
    for record in records.iter().filter(|record| record.is_active()) {
        if order.insert(record.state.as_str()) {
            seen.push((record.state, 1));
        } else if let Some(slot) = seen.iter_mut().find(|(state, _)| *state == record.state) {
            slot.1 += 1;
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use tempfile::TempDir;

    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 1, 9, 0, 0).single().unwrap()
    }

    fn registration(run_id: &str, thread: &str, meta: &Path) -> RunMonitorRegistration {
        RunMonitorRegistration {
            run_id: run_id.to_string(),
            thread_store_id: thread.to_string(),
            meta_path: meta.to_path_buf(),
            transcript_path: None,
            report_path: None,
            poll_interval_secs: 1,
            stale_after_secs: 60,
        }
    }

    #[test]
    fn store_registers_persists_and_resumes_across_instances() {
        let tmp = TempDir::new().unwrap();
        let store = RunMonitorStore::new_in(tmp.path()).unwrap();
        let record = store
            .register(
                registration("work-1", "t_thread", Path::new("/tmp/meta.json")),
                now(),
            )
            .unwrap();
        assert_eq!(record.state, RunMonitorState::Started);

        // A fresh store instance over the same directory (app restart) sees the
        // same active monitor — persisted checkpoint resumes observation.
        let resumed = RunMonitorStore::new_in(tmp.path()).unwrap();
        let active = resumed.active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].monitor_id, record.monitor_id);
        assert_eq!(active[0].run_id, "work-1");
    }

    #[test]
    fn register_is_idempotent_per_run_and_thread() {
        let tmp = TempDir::new().unwrap();
        let store = RunMonitorStore::new_in(tmp.path()).unwrap();
        let meta = tmp.path().join("meta.json");
        let first = store
            .register(registration("work-1", "t_thread", &meta), now())
            .unwrap();
        let second = store
            .register(registration("work-1", "t_thread", &meta), now())
            .unwrap();
        assert_eq!(first.monitor_id, second.monitor_id);
        assert_eq!(store.list().unwrap().len(), 1);

        // A different thread observing the same run is a distinct monitor.
        let other = store
            .register(registration("work-1", "t_other", &meta), now())
            .unwrap();
        assert_ne!(other.monitor_id, first.monitor_id);
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn cancel_keeps_inspectable_evidence_and_stops_activity() {
        let tmp = TempDir::new().unwrap();
        let store = RunMonitorStore::new_in(tmp.path()).unwrap();
        let record = store
            .register(
                registration("work-1", "t_thread", Path::new("/tmp/meta.json")),
                now(),
            )
            .unwrap();
        assert!(store.cancel(&record.monitor_id, now()).unwrap());
        assert!(store.active().unwrap().is_empty());
        let stored = store.get(&record.monitor_id).unwrap().unwrap();
        assert!(stored.cancelled);
        assert!(stored.done);
        assert!(!store.cancel("mon-missing", now()).unwrap());
    }

    #[test]
    fn classify_covers_the_full_state_machine() {
        // Live and progressing.
        assert_eq!(
            classify_run_state(Some("active"), None, false, false, 5, 300, true),
            RunMonitorState::Running
        );
        // Live, no progress yet observed.
        assert_eq!(
            classify_run_state(Some("active"), None, false, false, 5, 300, false),
            RunMonitorState::Started
        );
        // Silent past the stale bar without a report.
        assert_eq!(
            classify_run_state(Some("active"), None, false, false, 301, 300, false),
            RunMonitorState::Stale
        );
        // Meta stuck `active` but the promised report exists: completion skew.
        assert_eq!(
            classify_run_state(Some("active"), None, false, true, 301, 300, false),
            RunMonitorState::Completed
        );
        // Explicit block.
        assert_eq!(
            classify_run_state(Some("blocked"), None, false, false, 5, 300, true),
            RunMonitorState::Blocked
        );
        // Clean terminal with artifact.
        assert_eq!(
            classify_run_state(Some("completed"), Some(0), true, true, 5, 300, false),
            RunMonitorState::Completed
        );
        // Success claim without the promised artifact.
        assert_eq!(
            classify_run_state(Some("completed"), Some(0), true, false, 5, 300, false),
            RunMonitorState::NeedsRerun
        );
        // Non-zero exit dominates.
        assert_eq!(
            classify_run_state(Some("completed"), Some(3), true, true, 5, 300, false),
            RunMonitorState::Failed
        );
        assert_eq!(
            classify_run_state(Some("failed"), None, true, true, 5, 300, false),
            RunMonitorState::Failed
        );
    }

    fn record_with(last_notified: Option<RunMonitorState>) -> RunMonitorRecord {
        RunMonitorRecord {
            monitor_id: "mon-test".to_string(),
            run_id: "work-1".to_string(),
            thread_store_id: "t_thread".to_string(),
            meta_path: PathBuf::from("/tmp/meta.json"),
            transcript_path: None,
            report_path: None,
            state: RunMonitorState::Running,
            created_at: now(),
            updated_at: now(),
            last_notified_state: last_notified,
            done: false,
            cancelled: false,
            poll_interval_secs: 1,
            stale_after_secs: 60,
            consecutive_read_failures: 0,
            next_poll_at: now(),
            meta_len: 0,
            transcript_len: 0,
        }
    }

    #[test]
    fn heartbeat_decisions_dedupe_and_stay_meaningful() {
        // Quiet progress never notifies.
        assert_eq!(
            decide_heartbeat(&record_with(None), RunMonitorState::Running),
            None
        );
        assert_eq!(
            decide_heartbeat(&record_with(None), RunMonitorState::Started),
            None
        );
        // Stall notifies once.
        assert_eq!(
            decide_heartbeat(&record_with(None), RunMonitorState::Stale),
            Some(HeartbeatKind::Stalled)
        );
        assert_eq!(
            decide_heartbeat(
                &record_with(Some(RunMonitorState::Stale)),
                RunMonitorState::Stale
            ),
            None
        );
        // Recovery only after a notified degradation.
        assert_eq!(
            decide_heartbeat(
                &record_with(Some(RunMonitorState::Stale)),
                RunMonitorState::Running
            ),
            Some(HeartbeatKind::Recovered)
        );
        // Terminal states notify.
        assert_eq!(
            decide_heartbeat(&record_with(None), RunMonitorState::Completed),
            Some(HeartbeatKind::Completed)
        );
        assert_eq!(
            decide_heartbeat(&record_with(None), RunMonitorState::NeedsRerun),
            Some(HeartbeatKind::NeedsRerun)
        );
        // Done/cancelled monitors are silent forever.
        let mut done = record_with(None);
        done.done = true;
        assert_eq!(decide_heartbeat(&done, RunMonitorState::Failed), None);
        let mut cancelled = record_with(None);
        cancelled.cancelled = true;
        assert_eq!(
            decide_heartbeat(&cancelled, RunMonitorState::Completed),
            None
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(next_poll_delay_secs(5, 0), 5);
        assert_eq!(next_poll_delay_secs(5, 1), 10);
        assert_eq!(next_poll_delay_secs(5, 3), 40);
        assert_eq!(next_poll_delay_secs(5, 20), MAX_BACKOFF_SECS);
        assert_eq!(next_poll_delay_secs(0, 0), 1);
    }

    #[test]
    fn bounded_tail_clips_to_budget() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("transcript.log");
        fs::write(&path, "x".repeat(10_000)).unwrap();
        let tail = bounded_tail(&path, MAX_HEARTBEAT_EXCERPT_BYTES).unwrap();
        assert_eq!(tail.len(), MAX_HEARTBEAT_EXCERPT_BYTES);
        assert!(bounded_tail(&tmp.path().join("missing.log"), 64).is_none());
    }

    #[test]
    fn run_id_validation_rejects_traversal_shapes() {
        assert!(is_valid_run_id("work-260801-104012-91713"));
        assert!(!is_valid_run_id(""));
        assert!(!is_valid_run_id("../etc/passwd"));
        assert!(!is_valid_run_id("a/b"));
        assert!(!is_valid_run_id(".hidden"));
    }

    #[test]
    fn observe_reads_meta_and_flags_progress() {
        let tmp = TempDir::new().unwrap();
        let meta = tmp.path().join("meta.json");
        fs::write(&meta, r#"{"status":"active"}"#).unwrap();
        let mut record = record_with(None);
        record.meta_path = meta.clone();
        record.stale_after_secs = 3600;

        let first = observe_record(&record, Utc::now()).unwrap();
        assert_eq!(first.state, RunMonitorState::Running);
        record.meta_len = first.meta_len;

        // Unchanged meta: no progress, still live.
        let second = observe_record(&record, Utc::now()).unwrap();
        assert_eq!(second.state, RunMonitorState::Started);

        // Terminal meta with report artifact present.
        let report = tmp.path().join("report.md");
        fs::write(&report, "# done").unwrap();
        fs::write(
            &meta,
            format!(
                r#"{{"status":"completed","exit_code":0,"completed_at":"2026-08-01T09:00:00Z","report":{}}}"#,
                serde_json::to_string(report.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        let terminal = observe_record(&record, Utc::now()).unwrap();
        assert_eq!(terminal.state, RunMonitorState::Completed);
        assert!(terminal.report_present);
    }
}
