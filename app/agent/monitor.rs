//! Resident monitor service: the application-owned polling loop over
//! registered run monitors.
//!
//! The assistant turn only registers a monitor (`monitor_run` tool); this
//! service keeps observing after the turn ends, checkpoints every poll into the
//! durable [`RunMonitorStore`], and re-enters the registering thread through
//! the canonical [`ThreadDeliveryGateway`] when — and only when — the state
//! machine says the change is worth a heartbeat. Raw logs are never tailed
//! live; the control-plane `meta.json` is the cursor surface and transcript
//! tails are bounded terminal evidence.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::json;
use tracing::{info, warn};

use codescribe_core::agent::run_monitor::{
    DEFAULT_POLL_INTERVAL_SECS, DEFAULT_STALE_AFTER_SECS, MAX_HEARTBEAT_EXCERPT_BYTES,
    MAX_READ_FAILURES_BEFORE_BLOCKED, bounded_tail, decide_heartbeat, heartbeat_text,
    is_valid_run_id, next_poll_delay_secs, observe_record,
};
use codescribe_core::agent::{
    HeartbeatKind, RunMonitorRecord, RunMonitorRegistration, RunMonitorState, RunMonitorStore,
    ThreadDeliveryGateway, ThreadDeliveryInput, ThreadDeliverySource, ThreadMessage, ThreadStore,
};

/// Service loop wake cadence. Individual monitors gate themselves via
/// `next_poll_at`, so a short tick here does not imply per-run poll spam.
const SERVICE_TICK: Duration = Duration::from_secs(2);

/// Outcome counters for one pass over the active monitors (test surface).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickSummary {
    pub polled: usize,
    pub heartbeats: usize,
    pub finished: usize,
}

/// One poll pass over every due active monitor. Pure with respect to process
/// state: all stores are passed in, so tests run fully isolated.
pub fn tick_all(
    store: &RunMonitorStore,
    thread_store: &ThreadStore,
    gateway: &ThreadDeliveryGateway,
    now: DateTime<Utc>,
) -> Result<TickSummary> {
    let mut summary = TickSummary::default();
    for mut record in store.active()? {
        if record.next_poll_at > now {
            continue;
        }
        summary.polled += 1;

        match observe_record(&record, now) {
            Ok(observation) => {
                record.consecutive_read_failures = 0;
                record.state = observation.state;
                record.meta_len = observation.meta_len;
                record.transcript_len = observation.transcript_len;

                if let Some(kind) = decide_heartbeat(&record, observation.state) {
                    let excerpt = terminal_excerpt(kind, &record);
                    let text = heartbeat_text(kind, &record, &observation, excerpt.as_deref());
                    deliver_heartbeat(
                        thread_store,
                        gateway,
                        &record,
                        &text,
                        observation.state,
                        now,
                    )
                    .with_context(|| {
                        format!(
                            "Failed to deliver heartbeat for monitor {}",
                            record.monitor_id
                        )
                    })?;
                    record.last_notified_state = Some(observation.state);
                    summary.heartbeats += 1;
                    if observation.state.is_terminal() {
                        record.done = true;
                        summary.finished += 1;
                    }
                }
                record.next_poll_at =
                    now + chrono::Duration::seconds(record.poll_interval_secs as i64);
            }
            Err(error) => {
                record.consecutive_read_failures += 1;
                warn!(
                    monitor_id = %record.monitor_id,
                    run_id = %record.run_id,
                    failures = record.consecutive_read_failures,
                    %error,
                    "Run monitor observation failed"
                );
                if record.consecutive_read_failures >= MAX_READ_FAILURES_BEFORE_BLOCKED {
                    record.state = RunMonitorState::Blocked;
                    if decide_heartbeat(&record, RunMonitorState::Blocked).is_some() {
                        let text = format!(
                            "Run `{}` monitor is blocked: {} consecutive failures reading {}.",
                            record.run_id,
                            record.consecutive_read_failures,
                            record.meta_path.display()
                        );
                        deliver_heartbeat(
                            thread_store,
                            gateway,
                            &record,
                            &text,
                            RunMonitorState::Blocked,
                            now,
                        )?;
                        record.last_notified_state = Some(RunMonitorState::Blocked);
                        summary.heartbeats += 1;
                    }
                }
                let delay = next_poll_delay_secs(
                    record.poll_interval_secs,
                    record.consecutive_read_failures,
                );
                record.next_poll_at = now + chrono::Duration::seconds(delay as i64);
            }
        }

        record.updated_at = now;
        store.update(&record)?;
    }
    Ok(summary)
}

/// Bounded transcript tail for terminal heartbeats only — progress and stall
/// notices stay one-liners.
fn terminal_excerpt(kind: HeartbeatKind, record: &RunMonitorRecord) -> Option<String> {
    match kind {
        HeartbeatKind::Completed | HeartbeatKind::Failed | HeartbeatKind::NeedsRerun => record
            .transcript_path
            .as_deref()
            .and_then(|path| bounded_tail(path, MAX_HEARTBEAT_EXCERPT_BYTES)),
        _ => None,
    }
}

/// Append one heartbeat message to the existing thread through the canonical
/// delivery gateway. Append only — committed history is never replaced.
fn deliver_heartbeat(
    thread_store: &ThreadStore,
    gateway: &ThreadDeliveryGateway,
    record: &RunMonitorRecord,
    text: &str,
    state: RunMonitorState,
    now: DateTime<Utc>,
) -> Result<()> {
    let existing = if thread_store
        .thread_file_path(&record.thread_store_id)?
        .exists()
    {
        Some(thread_store.load_thread(&record.thread_store_id)?)
    } else {
        None
    };

    let heartbeat = ThreadMessage {
        role: "assistant".to_string(),
        content: vec![json!({"type": "text", "text": text})],
        timestamp: now,
        metadata: Some(json!({
            "kind": "run-monitor-heartbeat",
            "monitor_id": record.monitor_id,
            "run_id": record.run_id,
            "state": state.as_str(),
        })),
    };

    let (mut messages, provider, model, mode, tags) = match existing {
        Some(thread) => (
            thread.messages,
            thread.provider,
            thread.model,
            thread.mode,
            thread.tags,
        ),
        None => (
            Vec::new(),
            "run-monitor".to_string(),
            "run-monitor".to_string(),
            "assistive".to_string(),
            vec!["agent".to_string()],
        ),
    };
    messages.push(heartbeat);

    gateway.deliver(ThreadDeliveryInput {
        backend_id: record.thread_store_id.clone(),
        messages,
        provider,
        model,
        source: ThreadDeliverySource::Monitor,
        mode,
        tags,
        timestamp: now,
    })?;
    Ok(())
}

/// Resolve the Vibecrafted control-plane root. `VIBECRAFTED_CONTROL_PLANE_DIR`
/// overrides for tests and non-default installs.
pub fn control_plane_root() -> PathBuf {
    if let Ok(custom) = std::env::var("VIBECRAFTED_CONTROL_PLANE_DIR") {
        return PathBuf::from(custom);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".vibecrafted/control_plane")
}

/// Register a monitor for `run_id` heartbeating into `thread_store_id`, using
/// an explicit store + control-plane root (test-injectable core of
/// [`register_run_monitor`]).
fn validate_registration(run_id: &str, thread_store_id: &str) -> Result<()> {
    if !is_valid_run_id(run_id) {
        bail!("Invalid run id '{run_id}'");
    }
    if thread_store_id.trim().is_empty() || thread_store_id == "unbound" {
        bail!("No durable thread is bound to this turn; cannot register a monitor");
    }
    Ok(())
}

pub fn register_run_monitor_in(
    store: &RunMonitorStore,
    control_plane_root: &std::path::Path,
    run_id: &str,
    thread_store_id: &str,
    now: DateTime<Utc>,
) -> Result<RunMonitorRecord> {
    validate_registration(run_id, thread_store_id)?;
    let run_dir = control_plane_root.join("runtime_runs").join(run_id);
    let meta_path = run_dir.join("meta.json");
    if !meta_path.exists() {
        bail!(
            "Run '{run_id}' has no control-plane meta at {}",
            meta_path.display()
        );
    }

    // Prefill artifact paths from the meta snapshot when present; the observer
    // re-reads them every poll anyway, this only seeds the record.
    let snapshot: codescribe_core::agent::run_monitor::RunMetaSnapshot =
        std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

    let transcript_path = snapshot
        .transcript
        .map(PathBuf::from)
        .or_else(|| Some(run_dir.join("transcript.log")));
    let report_path = snapshot.report.map(PathBuf::from);

    store.register(
        RunMonitorRegistration {
            run_id: run_id.to_string(),
            thread_store_id: thread_store_id.to_string(),
            meta_path,
            transcript_path,
            report_path,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            stale_after_secs: DEFAULT_STALE_AFTER_SECS,
        },
        now,
    )
}

/// Product entrypoint: register against the default store and control plane,
/// then make sure the resident service loop is running.
pub fn register_run_monitor(run_id: &str, thread_store_id: &str) -> Result<RunMonitorRecord> {
    validate_registration(run_id, thread_store_id)?;
    let store = RunMonitorStore::new()?;
    let record = register_run_monitor_in(
        &store,
        &control_plane_root(),
        run_id,
        thread_store_id,
        Utc::now(),
    )?;
    ensure_monitor_service_started();
    Ok(record)
}

/// App-startup resume: if the durable store holds active monitors from a
/// previous process, restart the service loop so observation continues without
/// anyone prompting again. No-op when there is nothing to watch.
pub fn resume_monitors_on_startup() {
    match RunMonitorStore::new().and_then(|store| store.active()) {
        Ok(active) if !active.is_empty() => {
            info!(count = active.len(), "Resuming persisted run monitors");
            ensure_monitor_service_started();
        }
        Ok(_) => {}
        Err(error) => warn!(%error, "Failed to inspect run-monitor store on startup"),
    }
}

struct MonitorServiceHandle {
    stop: &'static AtomicBool,
    thread: std::sync::Mutex<Option<JoinHandle<()>>>,
}

static MONITOR_STOP: AtomicBool = AtomicBool::new(false);
static MONITOR_SERVICE: OnceLock<MonitorServiceHandle> = OnceLock::new();

/// Start the resident polling loop once per process. Called on registration
/// and from app startup so persisted monitors resume after a restart.
pub fn ensure_monitor_service_started() {
    MONITOR_SERVICE.get_or_init(|| {
        MONITOR_STOP.store(false, Ordering::SeqCst);
        let thread = std::thread::Builder::new()
            .name("codescribe-run-monitor".to_string())
            .spawn(monitor_loop)
            .map_err(|error| {
                warn!(%error, "Failed to spawn run-monitor service thread");
                error
            })
            .ok();
        MonitorServiceHandle {
            stop: &MONITOR_STOP,
            thread: std::sync::Mutex::new(thread),
        }
    });
}

/// Stop the resident loop and join it (app shutdown cleanup).
pub fn shutdown_monitor_service() {
    if let Some(handle) = MONITOR_SERVICE.get() {
        handle.stop.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = handle.thread.lock()
            && let Some(join) = slot.take()
        {
            let _ = join.join();
        }
    }
}

fn monitor_loop() {
    info!("Run-monitor service started");
    loop {
        if MONITOR_STOP.load(Ordering::SeqCst) {
            break;
        }
        if let Err(error) = tick_default() {
            warn!(%error, "Run-monitor tick failed");
        }
        std::thread::sleep(SERVICE_TICK);
    }
    info!("Run-monitor service stopped");
}

fn tick_default() -> Result<()> {
    let store = RunMonitorStore::new()?;
    if store.active()?.is_empty() {
        return Ok(());
    }
    let thread_store = ThreadStore::new()?;
    let gateway = ThreadDeliveryGateway::new()?;
    tick_all(&store, &thread_store, &gateway, Utc::now())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use codescribe_core::agent::run_monitor::RunMonitorState;

    struct Harness {
        _tmp: TempDir,
        store: RunMonitorStore,
        thread_store: ThreadStore,
        gateway: ThreadDeliveryGateway,
        control_root: PathBuf,
        run_dir: PathBuf,
    }

    fn harness(run_id: &str) -> Harness {
        let tmp = TempDir::new().unwrap();
        let store = RunMonitorStore::new_in(tmp.path().join("monitors")).unwrap();
        let threads_dir = tmp.path().join("threads");
        let thread_store = ThreadStore::new_in(&threads_dir).unwrap();
        let gateway = ThreadDeliveryGateway::new_in(&threads_dir).unwrap();
        let control_root = tmp.path().join("control_plane");
        let run_dir = control_root.join("runtime_runs").join(run_id);
        fs::create_dir_all(&run_dir).unwrap();
        Harness {
            _tmp: tmp,
            store,
            thread_store,
            gateway,
            control_root,
            run_dir,
        }
    }

    fn write_meta(harness: &Harness, body: &str) {
        fs::write(harness.run_dir.join("meta.json"), body).unwrap();
    }

    fn seed_thread(harness: &Harness, thread_id: &str) {
        let at = Utc::now();
        harness
            .gateway
            .deliver(ThreadDeliveryInput {
                backend_id: thread_id.to_string(),
                messages: vec![
                    ThreadMessage {
                        role: "user".to_string(),
                        content: vec![json!({"type":"text","text":"obserwuj ten run"})],
                        timestamp: at,
                        metadata: None,
                    },
                    ThreadMessage {
                        role: "assistant".to_string(),
                        content: vec![json!({"type":"text","text":"Monitoruję."})],
                        timestamp: at,
                        metadata: None,
                    },
                ],
                provider: "openai-responses".to_string(),
                model: "gpt-test".to_string(),
                source: ThreadDeliverySource::Composer,
                mode: "assistive".to_string(),
                tags: vec!["agent".to_string()],
                timestamp: at,
            })
            .unwrap();
    }

    #[test]
    fn launched_run_progresses_and_final_state_surfaces_without_prompting() {
        let run_id = "work-260801-000000-1";
        let thread_id = "t_2026-08-01_monitor";
        let harness = harness(run_id);
        seed_thread(&harness, thread_id);
        write_meta(&harness, r#"{"status":"active"}"#);

        // Registration during the interactive turn (tool path core).
        let record = register_run_monitor_in(
            &harness.store,
            &harness.control_root,
            run_id,
            thread_id,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(record.state, RunMonitorState::Started);

        // Live progress: polled, checkpointed, but no chat spam.
        let first = tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(first.polled, 1);
        assert_eq!(first.heartbeats, 0);
        let checkpoint = harness.store.get(&record.monitor_id).unwrap().unwrap();
        assert_eq!(checkpoint.state, RunMonitorState::Running);
        assert!(checkpoint.meta_len > 0, "cursor checkpoint must persist");

        // App restart: a fresh store over the same directory resumes the same
        // monitor from its persisted checkpoint.
        let resumed_store =
            RunMonitorStore::new_in(harness.store.file_path().parent().unwrap()).unwrap();
        assert_eq!(resumed_store.active().unwrap().len(), 1);

        // The run finishes and leaves its promised report artifact.
        let report = harness.run_dir.join("report.md");
        fs::write(&report, "# report\nfinal claim").unwrap();
        fs::write(harness.run_dir.join("transcript.log"), "line-1\nline-2\n").unwrap();
        write_meta(
            &harness,
            &format!(
                r#"{{"status":"completed","exit_code":0,"completed_at":"2026-08-01T09:10:00Z","report":{},"transcript":{}}}"#,
                serde_json::to_string(report.to_str().unwrap()).unwrap(),
                serde_json::to_string(harness.run_dir.join("transcript.log").to_str().unwrap())
                    .unwrap(),
            ),
        );

        let due = Utc::now() + chrono::Duration::seconds(60);
        let terminal =
            tick_all(&resumed_store, &harness.thread_store, &harness.gateway, due).unwrap();
        assert_eq!(terminal.heartbeats, 1);
        assert_eq!(terminal.finished, 1);

        // The heartbeat re-entered the EXISTING thread: appended, not replaced.
        let thread = harness.thread_store.load_thread(thread_id).unwrap();
        assert_eq!(thread.messages.len(), 3);
        assert_eq!(thread.messages[0].role, "user");
        let heartbeat = &thread.messages[2];
        assert_eq!(heartbeat.role, "assistant");
        let text = heartbeat.content[0]["text"].as_str().unwrap();
        assert!(
            text.contains(run_id),
            "heartbeat names the canonical run id"
        );
        assert!(
            text.contains("completed"),
            "final state is surfaced: {text}"
        );
        assert!(text.contains("report.md"), "report artifact is referenced");
        assert_eq!(
            heartbeat.metadata.as_ref().unwrap()["kind"],
            "run-monitor-heartbeat"
        );

        // Monitor is done and inspectable; further ticks never duplicate.
        let finished = resumed_store.get(&record.monitor_id).unwrap().unwrap();
        assert!(finished.done);
        assert_eq!(finished.state, RunMonitorState::Completed);
        let after = tick_all(
            &resumed_store,
            &harness.thread_store,
            &harness.gateway,
            due + chrono::Duration::seconds(120),
        )
        .unwrap();
        assert_eq!(after.polled, 0);
        assert_eq!(after.heartbeats, 0);
        assert_eq!(
            harness
                .thread_store
                .load_thread(thread_id)
                .unwrap()
                .messages
                .len(),
            3,
            "no duplicate notifications"
        );
    }

    #[test]
    fn failed_run_surfaces_failure_with_bounded_excerpt() {
        let run_id = "work-260801-000000-2";
        let thread_id = "t_2026-08-01_fail";
        let harness = harness(run_id);
        seed_thread(&harness, thread_id);
        let transcript = harness.run_dir.join("transcript.log");
        fs::write(&transcript, "boom ".repeat(2000)).unwrap();
        write_meta(
            &harness,
            &format!(
                r#"{{"status":"failed","exit_code":7,"transcript":{}}}"#,
                serde_json::to_string(transcript.to_str().unwrap()).unwrap()
            ),
        );

        register_run_monitor_in(
            &harness.store,
            &harness.control_root,
            run_id,
            thread_id,
            Utc::now(),
        )
        .unwrap();
        let summary = tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(summary.heartbeats, 1);

        let thread = harness.thread_store.load_thread(thread_id).unwrap();
        let text = thread.messages[2].content[0]["text"].as_str().unwrap();
        assert!(text.contains("failed (exit code 7)"));
        assert!(
            text.len() < MAX_HEARTBEAT_EXCERPT_BYTES + 512,
            "heartbeat payload stays bounded, got {} bytes",
            text.len()
        );
    }

    #[test]
    fn stall_then_recovery_notifies_each_transition_once() {
        let run_id = "work-260801-000000-3";
        let thread_id = "t_2026-08-01_stall";
        let harness = harness(run_id);
        seed_thread(&harness, thread_id);
        write_meta(&harness, r#"{"status":"active"}"#);

        let record = register_run_monitor_in(
            &harness.store,
            &harness.control_root,
            run_id,
            thread_id,
            Utc::now(),
        )
        .unwrap();
        // Tighten the stale bar so wall-clock mtime age crosses it.
        let mut tightened = harness.store.get(&record.monitor_id).unwrap().unwrap();
        tightened.stale_after_secs = 1;
        harness.store.update(&tightened).unwrap();

        // First tick observes progress (Running, silent).
        tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            Utc::now(),
        )
        .unwrap();

        // Well past the stale bar with no meta change → one stall heartbeat,
        // and a repeat tick must not duplicate it.
        let later = Utc::now() + chrono::Duration::seconds(120);
        let stalled = tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            later,
        )
        .unwrap();
        assert_eq!(stalled.heartbeats, 1);
        let again = tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            later + chrono::Duration::seconds(60),
        )
        .unwrap();
        assert_eq!(again.heartbeats, 0, "stall must notify exactly once");

        // Meta progresses again → recovery heartbeat.
        write_meta(
            &harness,
            r#"{"status":"active","note":"progressing again"}"#,
        );
        let recovered = tick_all(
            &harness.store,
            &harness.thread_store,
            &harness.gateway,
            later + chrono::Duration::seconds(121),
        )
        .unwrap();
        assert_eq!(recovered.heartbeats, 1);
        let thread = harness.thread_store.load_thread(thread_id).unwrap();
        let last = thread.messages.last().unwrap().content[0]["text"]
            .as_str()
            .unwrap();
        assert!(last.contains("recovered"), "got: {last}");
    }

    #[test]
    fn registration_rejects_unknown_runs_and_unbound_threads() {
        let harness = harness("work-known");
        write_meta(&harness, r#"{"status":"active"}"#);
        assert!(
            register_run_monitor_in(
                &harness.store,
                &harness.control_root,
                "work-known",
                "unbound",
                Utc::now()
            )
            .is_err()
        );
        assert!(
            register_run_monitor_in(
                &harness.store,
                &harness.control_root,
                "work-missing",
                "t_thread",
                Utc::now()
            )
            .is_err()
        );
        assert!(
            register_run_monitor_in(
                &harness.store,
                &harness.control_root,
                "../escape",
                "t_thread",
                Utc::now()
            )
            .is_err()
        );
    }
}
