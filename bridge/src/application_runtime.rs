//! Process-wide Tokio runtime owned by the Codescribe application.
//!
//! UniFFI's Tokio compatibility adapter is intentionally not the execution
//! authority for app work. Exported async functions use [`run`] to move their
//! root future onto this runtime immediately; the foreign executor only waits
//! for the resulting Tokio join handle.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::runtime::{Builder, Handle, Runtime};
use tokio::task::{JoinError, JoinHandle};

use crate::CsError;

const WORKER_ENV: &str = "CODESCRIBE_APP_RUNTIME_WORKERS";
const DEFAULT_WORKERS: usize = 4;
const MAX_WORKERS: usize = 16;
const WORKER_PREFIX: &str = "codescribe-app-worker-";
const WORKER_START_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Observable, content-free runtime lifecycle evidence for Swift and probes.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct CsApplicationRuntimeSnapshot {
    /// `not_started`, `running`, or `stopped`.
    pub state: String,
    /// Configured async worker count for this process.
    pub worker_count: u32,
    /// Named async workers observed entering the runtime.
    pub worker_names: Vec<String>,
    /// Named async workers observed leaving the runtime.
    pub stopped_worker_names: Vec<String>,
    /// Root bridge tasks currently owned by the runtime adapter.
    pub active_tasks: u64,
}

#[derive(Default)]
struct RuntimeState {
    runtime: Option<Runtime>,
    permanently_stopped: bool,
}

struct RuntimeMetrics {
    worker_count: usize,
    worker_names: Mutex<BTreeSet<String>>,
    stopped_worker_names: Mutex<BTreeSet<String>>,
    worker_started: Condvar,
    active_tasks: AtomicU64,
}

impl RuntimeMetrics {
    fn new(worker_count: usize) -> Self {
        Self {
            worker_count,
            worker_names: Mutex::new(BTreeSet::new()),
            stopped_worker_names: Mutex::new(BTreeSet::new()),
            worker_started: Condvar::new(),
            active_tasks: AtomicU64::new(0),
        }
    }

    fn is_async_worker_name(&self, name: &str) -> bool {
        name.strip_prefix(WORKER_PREFIX)
            .and_then(|suffix| suffix.parse::<usize>().ok())
            .is_some_and(|index| (1..=self.worker_count).contains(&index))
    }

    fn note_thread_start(&self) {
        let Some(name) = std::thread::current().name().map(str::to_string) else {
            return;
        };
        if !self.is_async_worker_name(&name) {
            return;
        }
        let mut names = self.worker_names.lock().unwrap_or_else(|e| e.into_inner());
        names.insert(name);
        self.worker_started.notify_all();
    }

    fn note_thread_stop(&self) {
        let Some(name) = std::thread::current().name().map(str::to_string) else {
            return;
        };
        if !self.is_async_worker_name(&name) {
            return;
        }
        self.stopped_worker_names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name);
    }

    fn reset_worker_lifecycle(&self) {
        self.worker_names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.stopped_worker_names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn wait_for_workers(&self, timeout: Duration) -> Result<(), CsError> {
        let deadline = Instant::now() + timeout;
        let mut names = self.worker_names.lock().unwrap_or_else(|e| e.into_inner());
        while names.len() < self.worker_count {
            let now = Instant::now();
            if now >= deadline {
                return Err(runtime_error(format!(
                    "application runtime started only {}/{} named workers",
                    names.len(),
                    self.worker_count
                )));
            }
            let wait = deadline.saturating_duration_since(now);
            let (next, _) = self
                .worker_started
                .wait_timeout(names, wait)
                .unwrap_or_else(|e| e.into_inner());
            names = next;
        }
        Ok(())
    }
}

/// Runtime plus lifecycle/measurement state. One global instance serves the
/// app; tests construct local instances so shutdown never poisons another test.
struct ApplicationRuntime {
    state: Mutex<RuntimeState>,
    metrics: Arc<RuntimeMetrics>,
}

impl ApplicationRuntime {
    fn new(worker_count: usize) -> Self {
        Self {
            state: Mutex::new(RuntimeState::default()),
            metrics: Arc::new(RuntimeMetrics::new(worker_count)),
        }
    }

    fn start(&self) -> Result<CsApplicationRuntimeSnapshot, CsError> {
        self.start_with_policy(self.metrics.worker_count, WORKER_START_TIMEOUT)
    }

    fn start_with_policy(
        &self,
        worker_threads: usize,
        worker_start_timeout: Duration,
    ) -> Result<CsApplicationRuntimeSnapshot, CsError> {
        let mut state = self.state.lock().map_err(|_| {
            runtime_error("application runtime lifecycle lock poisoned".to_string())
        })?;
        if state.permanently_stopped {
            return Err(runtime_error(
                "application runtime cannot restart after shutdown".to_string(),
            ));
        }
        if state.runtime.is_some() {
            return Ok(self.snapshot_for(&state));
        }

        self.metrics.reset_worker_lifecycle();
        let name_counter = Arc::new(AtomicUsize::new(0));
        let name_counter_for_runtime = Arc::clone(&name_counter);
        let metrics_for_start = Arc::clone(&self.metrics);
        let metrics_for_stop = Arc::clone(&self.metrics);
        let runtime = Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .thread_name_fn(move || {
                let index = name_counter_for_runtime.fetch_add(1, Ordering::SeqCst) + 1;
                format!("{WORKER_PREFIX}{index}")
            })
            .on_thread_start(move || metrics_for_start.note_thread_start())
            .on_thread_stop(move || metrics_for_stop.note_thread_stop())
            .enable_all()
            .build()
            .map_err(|error| {
                runtime_error(format!(
                    "application runtime initialization failed: {error}"
                ))
            })?;
        state.runtime = Some(runtime);
        if let Err(error) = self.metrics.wait_for_workers(worker_start_timeout) {
            if let Some(runtime) = state.runtime.take() {
                runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
            }
            return Err(error);
        }
        let snapshot = self.snapshot_for(&state);
        tracing::info!(
            worker_count = snapshot.worker_count,
            worker_names = ?snapshot.worker_names,
            "Codescribe application runtime started"
        );
        Ok(snapshot)
    }

    fn handle(&self) -> Result<Handle, CsError> {
        self.start()?;
        let state = self.state.lock().map_err(|_| {
            runtime_error("application runtime lifecycle lock poisoned".to_string())
        })?;
        state
            .runtime
            .as_ref()
            .map(Runtime::handle)
            .cloned()
            .ok_or_else(|| runtime_error("application runtime is not running".to_string()))
    }

    fn spawn<F, T>(&self, future: F) -> Result<AbortOnDropTask<T>, CsError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.start()?;
        let state = self.state.lock().map_err(|_| {
            runtime_error("application runtime lifecycle lock poisoned".to_string())
        })?;
        let runtime = state
            .runtime
            .as_ref()
            .ok_or_else(|| runtime_error("application runtime is not running".to_string()))?;
        self.metrics.active_tasks.fetch_add(1, Ordering::SeqCst);
        let metrics = Arc::clone(&self.metrics);
        let handle = runtime.handle().spawn(async move {
            let _guard = ActiveTaskGuard { metrics };
            future.await
        });
        Ok(AbortOnDropTask {
            handle: Some(handle),
        })
    }

    async fn run<F, T>(&self, future: F) -> Result<T, CsError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn(future)?.join().await.map_err(join_error)
    }

    fn block_on<F, T>(&self, future: F) -> Result<T, CsError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let task = self.spawn(future)?;
        let handle = self.handle()?;
        std::thread::Builder::new()
            .name("codescribe-app-runtime-waiter".to_string())
            .spawn(move || handle.block_on(task.join()))
            .map_err(|error| {
                runtime_error(format!(
                    "application runtime waiter failed to start: {error}"
                ))
            })?
            .join()
            .map_err(|_| runtime_error("application runtime waiter panicked".to_string()))?
            .map_err(join_error)
    }

    fn shutdown(&self) -> Result<CsApplicationRuntimeSnapshot, CsError> {
        let runtime = {
            let mut state = self.state.lock().map_err(|_| {
                runtime_error("application runtime lifecycle lock poisoned".to_string())
            })?;
            if state.permanently_stopped {
                return Ok(self.snapshot_for(&state));
            }
            state.permanently_stopped = true;
            state.runtime.take()
        };

        if let Some(runtime) = runtime {
            runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
        }
        let state = self.state.lock().map_err(|_| {
            runtime_error("application runtime lifecycle lock poisoned".to_string())
        })?;
        let snapshot = self.snapshot_for(&state);
        tracing::info!(
            active_tasks = snapshot.active_tasks,
            stopped_workers = ?snapshot.stopped_worker_names,
            "Codescribe application runtime stopped"
        );
        Ok(snapshot)
    }

    fn snapshot(&self) -> Result<CsApplicationRuntimeSnapshot, CsError> {
        let state = self.state.lock().map_err(|_| {
            runtime_error("application runtime lifecycle lock poisoned".to_string())
        })?;
        Ok(self.snapshot_for(&state))
    }

    fn snapshot_for(&self, state: &RuntimeState) -> CsApplicationRuntimeSnapshot {
        let lifecycle = if state.runtime.is_some() {
            "running"
        } else if state.permanently_stopped {
            "stopped"
        } else {
            "not_started"
        };
        CsApplicationRuntimeSnapshot {
            state: lifecycle.to_string(),
            worker_count: self.metrics.worker_count as u32,
            worker_names: self
                .metrics
                .worker_names
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect(),
            stopped_worker_names: self
                .metrics
                .stopped_worker_names
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect(),
            active_tasks: self.metrics.active_tasks.load(Ordering::SeqCst),
        }
    }
}

struct ActiveTaskGuard {
    metrics: Arc<RuntimeMetrics>,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        self.metrics.active_tasks.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Dropping the foreign wait future aborts its application-owned root task.
/// This prevents a cancelled Swift `Task` from silently detaching bridge work.
struct AbortOnDropTask<T> {
    handle: Option<JoinHandle<T>>,
}

impl<T> AbortOnDropTask<T> {
    async fn join(mut self) -> Result<T, JoinError> {
        let result = self.handle.as_mut().expect("task handle must exist").await;
        self.handle.take();
        result
    }
}

impl<T> Drop for AbortOnDropTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

fn configured_worker_count_from(value: Option<&str>) -> usize {
    value
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|count| (1..=MAX_WORKERS).contains(count))
        .unwrap_or(DEFAULT_WORKERS)
}

fn configured_worker_count() -> usize {
    let raw = std::env::var(WORKER_ENV).ok();
    let count = configured_worker_count_from(raw.as_deref());
    if raw.as_deref().is_some_and(|value| {
        value
            .trim()
            .parse::<usize>()
            .ok()
            .is_none_or(|parsed| !(1..=MAX_WORKERS).contains(&parsed))
    }) {
        tracing::warn!(
            env = WORKER_ENV,
            value = ?raw,
            fallback = DEFAULT_WORKERS,
            "invalid application runtime worker count"
        );
    }
    count
}

fn global() -> &'static ApplicationRuntime {
    static APPLICATION_RUNTIME: OnceLock<ApplicationRuntime> = OnceLock::new();
    APPLICATION_RUNTIME.get_or_init(|| ApplicationRuntime::new(configured_worker_count()))
}

fn runtime_error(msg: String) -> CsError {
    CsError::Runtime { msg }
}

fn join_error(error: JoinError) -> CsError {
    runtime_error(format!("application runtime task failed: {error}"))
}

pub(crate) fn start() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    global().start()
}

pub(crate) async fn run<F, T>(future: F) -> Result<T, CsError>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    global().run(future).await
}

pub(crate) fn block_on<F, T>(future: F) -> Result<T, CsError>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    global().block_on(future)
}

pub(crate) fn snapshot() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    global().snapshot()
}

pub(crate) fn shutdown() -> Result<CsApplicationRuntimeSnapshot, CsError> {
    global().shutdown()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    struct DropWitness(Arc<AtomicBool>);

    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn application_runtime_owns_uniffi_async_exports() {
        let runtime = ApplicationRuntime::new(DEFAULT_WORKERS);
        let started = runtime.start().expect("runtime starts");
        assert_eq!(started.worker_count, 4);
        assert_eq!(
            started.worker_names,
            (1..=4)
                .map(|index| format!("{WORKER_PREFIX}{index}"))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "runtime_probe:start state={} workers={:?}",
            started.state, started.worker_names
        );

        let thread_name = runtime
            .block_on(async {
                std::thread::current()
                    .name()
                    .unwrap_or("unnamed")
                    .to_string()
            })
            .expect("task executes");
        assert!(thread_name.starts_with(WORKER_PREFIX), "{thread_name}");
        assert!(!thread_name.contains("async-compat"), "{thread_name}");

        for source in [
            include_str!("agent.rs"),
            include_str!("hotkeys.rs"),
            include_str!("recording.rs"),
        ] {
            assert!(
                !source.contains("async_runtime = \"tokio\""),
                "UniFFI Tokio fallback must not own exported futures"
            );
            assert!(
                source.contains("application_runtime::run"),
                "every async export module must route through the app runtime"
            );
        }

        let stopped = runtime.shutdown().expect("runtime shuts down");
        eprintln!(
            "runtime_probe:stop state={} active_tasks={} stopped_workers={:?}",
            stopped.state, stopped.active_tasks, stopped.stopped_worker_names
        );
        assert_eq!(stopped.active_tasks, 0);
        assert_eq!(stopped.stopped_worker_names, stopped.worker_names);
    }

    #[test]
    fn cancelled_waiter_and_shutdown_leave_no_owned_tasks() {
        let runtime = ApplicationRuntime::new(DEFAULT_WORKERS);
        runtime.start().expect("runtime starts");
        let dropped = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let witness = Arc::clone(&dropped);
        let task = runtime
            .spawn(async move {
                let _witness = DropWitness(witness);
                started_tx.send(()).expect("signal task start");
                std::future::pending::<()>().await;
            })
            .expect("task spawns");
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("task reached runtime");
        drop(task);

        let deadline = Instant::now() + Duration::from_secs(1);
        while (!dropped.load(Ordering::SeqCst)
            || runtime.snapshot().expect("snapshot").active_tasks != 0)
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(runtime.snapshot().expect("snapshot").active_tasks, 0);

        let stopped = runtime.shutdown().expect("runtime shuts down");
        assert_eq!(stopped.state, "stopped");
        assert_eq!(stopped.active_tasks, 0);
        assert_eq!(stopped.stopped_worker_names, stopped.worker_names);
        assert!(runtime.start().is_err(), "shutdown is terminal");
    }

    #[test]
    fn failed_worker_start_rolls_back_and_allows_a_real_retry() {
        let runtime = ApplicationRuntime::new(2);
        let error = runtime
            .start_with_policy(1, Duration::from_millis(25))
            .expect_err("one worker cannot satisfy a two-worker start policy");
        let CsError::Runtime { msg } = error else {
            panic!("worker start failure must be a runtime error");
        };
        assert!(msg.contains("only 1/2 named workers"), "{msg}");
        assert_eq!(
            runtime.snapshot().expect("snapshot").state,
            "not_started",
            "failed start must not leave a false running runtime"
        );

        let started = runtime.start().expect("retry starts the full runtime");
        assert_eq!(started.worker_names.len(), 2);
        runtime.shutdown().expect("retry runtime shuts down");
    }

    #[test]
    fn worker_policy_defaults_to_four_and_bounds_overrides() {
        assert_eq!(configured_worker_count_from(None), 4);
        assert_eq!(configured_worker_count_from(Some("")), 4);
        assert_eq!(configured_worker_count_from(Some("0")), 4);
        assert_eq!(configured_worker_count_from(Some("17")), 4);
        assert_eq!(configured_worker_count_from(Some("8")), 8);
    }
}
