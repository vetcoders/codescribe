use crate::pipeline::contracts::RawTranscript;
use anyhow::{Result, anyhow};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

const REFINE_SUPERSEDED_ERR: &str = "STT refine request superseded by a newer pending refine";
const LIVE_SUPERSEDED_ERR: &str = "STT live request superseded by a newer pending live preview";
const SHUTDOWN_ERR: &str = "STT scheduler is shutting down";
const DEFAULT_LIVE_MIN_INTERVAL_MS: u64 = 90;
const DEFAULT_COMMIT_MIN_INTERVAL_MS: u64 = 180;
const DEFAULT_REFINE_MIN_INTERVAL_MS: u64 = 240;
const DEFAULT_THERMAL_NOMINAL_MULT: f32 = 0.0;
const DEFAULT_THERMAL_FAIR_MULT: f32 = 1.0;
const DEFAULT_THERMAL_SERIOUS_MULT: f32 = 2.0;
const DEFAULT_THERMAL_CRITICAL_MULT: f32 = 4.0;

type InferFn = Arc<
    dyn Fn(Vec<f32>, u32, Option<String>, Option<String>) -> Result<RawTranscript> + Send + Sync,
>;
type CommitPrefilterFn = Arc<dyn Fn(&[f32], u32) -> Vec<f32> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SttLane {
    Live,
    /// Final utterance path. Scheduler always applies VAD prefilter first;
    /// empty speech extraction returns an empty transcript without inference.
    /// This path is unconditional: no env/runtime kill-switch.
    Commit,
    Refine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThermalLevel {
    Nominal,
    Fair,
    Serious,
    Critical,
}

impl ThermalLevel {
    fn as_u8(self) -> u8 {
        match self {
            Self::Nominal => 0,
            Self::Fair => 1,
            Self::Serious => 2,
            Self::Critical => 3,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Fair,
            2 => Self::Serious,
            3 => Self::Critical,
            _ => Self::Nominal,
        }
    }
}

static PROCESS_THERMAL_LEVEL: AtomicU64 = AtomicU64::new(0);
static SCHEDULER_REGISTRY: OnceLock<StdMutex<Vec<mpsc::UnboundedSender<SchedulerCommand>>>> =
    OnceLock::new();

pub fn current_process_thermal_level() -> ThermalLevel {
    ThermalLevel::from_u8(PROCESS_THERMAL_LEVEL.load(Ordering::Relaxed) as u8)
}

pub fn set_process_thermal_level(level: ThermalLevel) {
    PROCESS_THERMAL_LEVEL.store(u64::from(level.as_u8()), Ordering::Relaxed);
    if let Some(registry) = SCHEDULER_REGISTRY.get() {
        let mut senders = registry.lock().unwrap_or_else(|e| e.into_inner());
        senders.retain(|tx| tx.send(SchedulerCommand::SetThermalLevel(level)).is_ok());
    }
}

#[derive(Debug)]
pub(crate) struct SttTaskHandle {
    id: u64,
    lane: SttLane,
    result_rx: oneshot::Receiver<Result<RawTranscript>>,
}

impl SttTaskHandle {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn lane(&self) -> SttLane {
        self.lane
    }

    pub(crate) async fn recv(&mut self) -> Result<RawTranscript> {
        match (&mut self.result_rx).await {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "STT scheduler response channel closed (lane={:?}, id={})",
                self.lane,
                self.id
            )),
        }
    }
}

pub(crate) struct SttScheduler {
    command_tx: mpsc::UnboundedSender<SchedulerCommand>,
    worker_handle: Option<JoinHandle<()>>,
    next_request_id: AtomicU64,
}

impl SttScheduler {
    pub(crate) fn new() -> Self {
        Self::with_runtime_fns(Arc::new(default_infer), Arc::new(default_commit_prefilter))
    }

    pub(crate) fn submit(
        &self,
        lane: SttLane,
        samples: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
    ) -> Result<SttTaskHandle> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (result_tx, result_rx) = oneshot::channel();
        let request = SttRequest {
            lane,
            samples,
            sample_rate,
            language,
            utterance_id: None,
            initial_prompt: initial_prompt_for_lane(lane),
            queued_at: Instant::now(),
            result_tx,
        };
        self.command_tx
            .send(SchedulerCommand::Submit(request))
            .map_err(|_| anyhow!("Failed to submit STT request: scheduler worker is closed"))?;
        Ok(SttTaskHandle {
            id: request_id,
            lane,
            result_rx,
        })
    }

    pub(crate) fn submit_for_utterance(
        &self,
        lane: SttLane,
        samples: Vec<f32>,
        sample_rate: u32,
        language: Option<String>,
        utterance_id: u64,
    ) -> Result<SttTaskHandle> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (result_tx, result_rx) = oneshot::channel();
        let request = SttRequest {
            lane,
            samples,
            sample_rate,
            language,
            utterance_id: Some(utterance_id),
            initial_prompt: initial_prompt_for_lane(lane),
            queued_at: Instant::now(),
            result_tx,
        };
        self.command_tx
            .send(SchedulerCommand::Submit(request))
            .map_err(|_| anyhow!("Failed to submit STT request: scheduler worker is closed"))?;
        Ok(SttTaskHandle {
            id: request_id,
            lane,
            result_rx,
        })
    }

    #[cfg(test)]
    pub(crate) fn set_thermal_level(&self, level: ThermalLevel) -> Result<()> {
        self.command_tx
            .send(SchedulerCommand::SetThermalLevel(level))
            .map_err(|_| anyhow!("Failed to set thermal level: scheduler worker is closed"))
    }

    pub(crate) async fn shutdown(mut self) -> Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.command_tx.send(SchedulerCommand::Shutdown { ack_tx });
        drop(self.command_tx);

        let _ = ack_rx.await;

        if let Some(handle) = self.worker_handle.take() {
            handle
                .await
                .map_err(|e| anyhow!("STT scheduler worker join error: {}", e))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn with_infer_fn(infer_fn: InferFn) -> Self {
        Self::with_runtime_fns_and_test_governor(
            infer_fn,
            Arc::new(default_commit_prefilter),
            SttGovernorConfig::test(0, 0),
        )
    }

    #[cfg(test)]
    pub(crate) fn with_infer_and_commit_prefilter(
        infer_fn: InferFn,
        commit_prefilter_fn: CommitPrefilterFn,
    ) -> Self {
        Self::with_runtime_fns_and_test_governor(
            infer_fn,
            commit_prefilter_fn,
            SttGovernorConfig::test(0, 0),
        )
    }

    fn with_runtime_fns(infer_fn: InferFn, commit_prefilter_fn: CommitPrefilterFn) -> Self {
        Self::with_runtime_fns_and_governor(
            infer_fn,
            commit_prefilter_fn,
            SttGovernorConfig::from_env(),
        )
    }

    #[cfg(test)]
    fn with_runtime_fns_and_test_governor(
        infer_fn: InferFn,
        commit_prefilter_fn: CommitPrefilterFn,
        governor: SttGovernorConfig,
    ) -> Self {
        Self::with_runtime_fns_and_governor(infer_fn, commit_prefilter_fn, governor)
    }

    fn with_runtime_fns_and_governor(
        infer_fn: InferFn,
        commit_prefilter_fn: CommitPrefilterFn,
        governor: SttGovernorConfig,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        SCHEDULER_REGISTRY
            .get_or_init(|| StdMutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command_tx.clone());
        let worker_handle = tokio::spawn(scheduler_worker(
            command_rx,
            infer_fn,
            commit_prefilter_fn,
            governor,
        ));
        Self {
            command_tx,
            worker_handle: Some(worker_handle),
            next_request_id: AtomicU64::new(0),
        }
    }
}

fn default_infer(
    samples: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
    initial_prompt: Option<String>,
) -> Result<RawTranscript> {
    crate::stt::transcribe_long_with_segments_with_initial_prompt(
        &samples,
        sample_rate,
        language.as_deref(),
        initial_prompt,
    )
}

fn initial_prompt_for_lane(lane: SttLane) -> Option<String> {
    match lane {
        SttLane::Live => None,
        SttLane::Commit | SttLane::Refine => {
            crate::pipeline::stream_postprocess::whisper_initial_prompt()
        }
    }
}

fn default_commit_prefilter(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    // Commit/final lane: trim only LEADING/TRAILING silence and keep every
    // interior window. Using `extract_speech` here (which concatenates only the
    // speech windows) would excise interior windows that dip below threshold —
    // e.g. the micro-pause between two spoken digits — dropping mid-utterance
    // speech from the AUTHORITATIVE transcript. `extract_speech_trim_edges`
    // returns the contiguous first..=last speech slab instead, so words are
    // never lost mid-utterance; it still returns empty on genuine silence.
    crate::vad::extract_speech_trim_edges(samples, sample_rate)
}

struct SttRequest {
    lane: SttLane,
    samples: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
    utterance_id: Option<u64>,
    initial_prompt: Option<String>,
    queued_at: Instant,
    result_tx: oneshot::Sender<Result<RawTranscript>>,
}

enum SchedulerCommand {
    Submit(SttRequest),
    SetThermalLevel(ThermalLevel),
    Shutdown { ack_tx: oneshot::Sender<()> },
}

#[derive(Clone, Debug)]
struct SttGovernorConfig {
    enabled: bool,
    live_min_interval: Duration,
    commit_min_interval: Duration,
    refine_min_interval: Duration,
    nominal_multiplier: f32,
    fair_multiplier: f32,
    serious_multiplier: f32,
    critical_multiplier: f32,
}

impl SttGovernorConfig {
    fn from_env() -> Self {
        Self {
            enabled: env_bool("CODESCRIBE_STT_THERMAL_GOVERNOR_ENABLED", true),
            live_min_interval: Duration::from_millis(env_u64(
                "CODESCRIBE_STT_MIN_INFER_INTERVAL_MS",
                DEFAULT_LIVE_MIN_INTERVAL_MS,
            )),
            commit_min_interval: Duration::from_millis(env_u64(
                "CODESCRIBE_STT_COMMIT_MIN_INTERVAL_MS",
                DEFAULT_COMMIT_MIN_INTERVAL_MS,
            )),
            refine_min_interval: Duration::from_millis(env_u64(
                "CODESCRIBE_STT_REFINE_MIN_INTERVAL_MS",
                DEFAULT_REFINE_MIN_INTERVAL_MS,
            )),
            nominal_multiplier: env_f32(
                "CODESCRIBE_STT_THERMAL_NOMINAL_MULT",
                DEFAULT_THERMAL_NOMINAL_MULT,
            )
            .max(0.0),
            fair_multiplier: env_f32(
                "CODESCRIBE_STT_THERMAL_FAIR_MULT",
                DEFAULT_THERMAL_FAIR_MULT,
            )
            .max(0.1),
            serious_multiplier: env_f32(
                "CODESCRIBE_STT_THERMAL_SERIOUS_MULT",
                DEFAULT_THERMAL_SERIOUS_MULT,
            )
            .max(0.1),
            critical_multiplier: env_f32(
                "CODESCRIBE_STT_THERMAL_CRITICAL_MULT",
                DEFAULT_THERMAL_CRITICAL_MULT,
            )
            .max(0.1),
        }
    }

    #[cfg(test)]
    fn test(live_ms: u64, commit_ms: u64) -> Self {
        Self::test_with_intervals(live_ms, commit_ms, commit_ms)
    }

    #[cfg(test)]
    fn test_with_intervals(live_ms: u64, commit_ms: u64, refine_ms: u64) -> Self {
        Self {
            enabled: true,
            live_min_interval: Duration::from_millis(live_ms),
            commit_min_interval: Duration::from_millis(commit_ms),
            refine_min_interval: Duration::from_millis(refine_ms),
            nominal_multiplier: DEFAULT_THERMAL_NOMINAL_MULT,
            fair_multiplier: DEFAULT_THERMAL_FAIR_MULT,
            serious_multiplier: DEFAULT_THERMAL_SERIOUS_MULT,
            critical_multiplier: DEFAULT_THERMAL_CRITICAL_MULT,
        }
    }

    fn min_interval_for(&self, lane: SttLane, level: ThermalLevel) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }
        let base = match lane {
            SttLane::Live => self.live_min_interval,
            SttLane::Commit => self.commit_min_interval,
            SttLane::Refine => self.refine_min_interval,
        };
        base.mul_f32(match level {
            ThermalLevel::Nominal => self.nominal_multiplier,
            ThermalLevel::Fair => self.fair_multiplier,
            ThermalLevel::Serious => self.serious_multiplier,
            ThermalLevel::Critical => self.critical_multiplier,
        })
    }
}

async fn scheduler_worker(
    mut command_rx: mpsc::UnboundedReceiver<SchedulerCommand>,
    infer_fn: InferFn,
    commit_prefilter_fn: CommitPrefilterFn,
    governor: SttGovernorConfig,
) {
    let mut live_queue: VecDeque<SttRequest> = VecDeque::new();
    let mut commit_queue: VecDeque<SttRequest> = VecDeque::new();
    let mut refine_pending: Option<SttRequest> = None;
    let mut thermal_level = current_process_thermal_level();
    let mut last_infer_completed_at: Option<Instant> = None;
    let mut is_shutting_down = false;
    let mut shutdown_ack: Option<oneshot::Sender<()>> = None;

    loop {
        if live_queue.is_empty() && commit_queue.is_empty() && refine_pending.is_none() {
            if is_shutting_down {
                break;
            }
            match command_rx.recv().await {
                Some(cmd) => handle_command(
                    cmd,
                    &mut live_queue,
                    &mut commit_queue,
                    &mut refine_pending,
                    &mut thermal_level,
                    &mut is_shutting_down,
                    &mut shutdown_ack,
                ),
                None => {
                    is_shutting_down = true;
                    continue;
                }
            }
        }

        drain_commands(
            &mut command_rx,
            &mut live_queue,
            &mut commit_queue,
            &mut refine_pending,
            &mut thermal_level,
            &mut is_shutting_down,
            &mut shutdown_ack,
        );

        if let Some(req) = pop_next_request(
            &mut live_queue,
            &mut commit_queue,
            &mut refine_pending,
            thermal_level,
        ) {
            let dequeued_at = Instant::now();
            let lane = req.lane;
            let utterance_id = req.utterance_id;
            let queue_ms = dequeued_at.duration_since(req.queued_at).as_millis();
            let duty_sleep =
                sleep_for_duty_cycle(&governor, thermal_level, lane, last_infer_completed_at).await;
            let infer = Arc::clone(&infer_fn);
            let commit_prefilter = Arc::clone(&commit_prefilter_fn);
            let infer_started_at = Instant::now();
            let result = tokio::task::spawn_blocking(move || {
                let samples = if lane == SttLane::Commit {
                    // Commit contract (hard): always VAD-prefilter before inference.
                    // Never add env/runtime bypasses that allow raw commit passthrough.
                    // The Commit lane must remain deterministic and VAD-first.
                    //
                    // P3.9 note (intentional duplicate VAD): the interim lane also
                    // runs extract_speech on overlapping audio. Reusing that result
                    // here is NOT done deliberately — the interim pass extracts
                    // speech over accumulated *sub-buffers*, whereas this prefilter
                    // must extract over the *whole* utterance as one deterministic,
                    // self-contained pass (the invariant above). The two are not
                    // substitutable. The cheap part — Silero VAD sample counts
                    // (`speech_vad_samples`) — is already reused upstream for the
                    // silence-drop gate and telemetry; only the full speech-sample
                    // extraction is recomputed here, and that recompute is required
                    // to keep the Commit lane independent of interim chunking.
                    let speech = (commit_prefilter)(&req.samples, req.sample_rate);
                    if speech.is_empty() {
                        tracing::info!(
                            "Commit VAD: no speech in {:.1}s utterance — returning empty",
                            req.samples.len() as f32 / req.sample_rate as f32
                        );
                        return Ok(RawTranscript {
                            text: String::new(),
                            segments: Vec::new(),
                            ..Default::default()
                        });
                    }
                    tracing::debug!(
                        "Commit VAD: {:.1}s speech / {:.1}s total ({:.0}% speech)",
                        speech.len() as f32 / req.sample_rate as f32,
                        req.samples.len() as f32 / req.sample_rate as f32,
                        (speech.len() as f32 / req.samples.len().max(1) as f32) * 100.0,
                    );
                    speech
                } else {
                    req.samples
                };
                (infer)(samples, req.sample_rate, req.language, req.initial_prompt)
            })
            .await
            .map_err(|e| anyhow!("STT blocking worker task failed: {}", e))
            .and_then(|r| r);
            let infer_elapsed = infer_started_at.elapsed();
            last_infer_completed_at = Some(Instant::now());
            tracing::debug!(
                ?lane,
                ?thermal_level,
                ?utterance_id,
                queue_ms,
                duty_sleep_ms = duty_sleep.as_millis(),
                infer_ms = infer_elapsed.as_millis(),
                ok = result.is_ok(),
                "STT scheduler lane timing"
            );
            let _ = req.result_tx.send(result);
            continue;
        }

        if is_shutting_down {
            break;
        }

        match command_rx.recv().await {
            Some(cmd) => handle_command(
                cmd,
                &mut live_queue,
                &mut commit_queue,
                &mut refine_pending,
                &mut thermal_level,
                &mut is_shutting_down,
                &mut shutdown_ack,
            ),
            None => is_shutting_down = true,
        }
    }

    // Any leftovers are canceled deterministically.
    for req in live_queue {
        let _ = req.result_tx.send(Err(anyhow!(SHUTDOWN_ERR)));
    }
    for req in commit_queue {
        let _ = req.result_tx.send(Err(anyhow!(SHUTDOWN_ERR)));
    }
    if let Some(req) = refine_pending.take() {
        let _ = req.result_tx.send(Err(anyhow!(SHUTDOWN_ERR)));
    }

    if let Some(ack_tx) = shutdown_ack {
        let _ = ack_tx.send(());
    }
}

fn drain_commands(
    command_rx: &mut mpsc::UnboundedReceiver<SchedulerCommand>,
    live_queue: &mut VecDeque<SttRequest>,
    commit_queue: &mut VecDeque<SttRequest>,
    refine_pending: &mut Option<SttRequest>,
    thermal_level: &mut ThermalLevel,
    is_shutting_down: &mut bool,
    shutdown_ack: &mut Option<oneshot::Sender<()>>,
) {
    loop {
        match command_rx.try_recv() {
            Ok(cmd) => handle_command(
                cmd,
                live_queue,
                commit_queue,
                refine_pending,
                thermal_level,
                is_shutting_down,
                shutdown_ack,
            ),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *is_shutting_down = true;
                break;
            }
        }
    }
}

fn handle_command(
    cmd: SchedulerCommand,
    live_queue: &mut VecDeque<SttRequest>,
    commit_queue: &mut VecDeque<SttRequest>,
    refine_pending: &mut Option<SttRequest>,
    thermal_level: &mut ThermalLevel,
    is_shutting_down: &mut bool,
    shutdown_ack: &mut Option<oneshot::Sender<()>>,
) {
    match cmd {
        SchedulerCommand::Submit(req) => {
            if *is_shutting_down {
                let _ = req.result_tx.send(Err(anyhow!(SHUTDOWN_ERR)));
                return;
            }
            enqueue_request(req, live_queue, commit_queue, refine_pending);
        }
        SchedulerCommand::SetThermalLevel(level) => {
            if *thermal_level != level {
                tracing::warn!(?level, "STT scheduler thermal level updated");
            }
            *thermal_level = level;
        }
        SchedulerCommand::Shutdown { ack_tx } => {
            *is_shutting_down = true;
            if let Some(prev) = shutdown_ack.replace(ack_tx) {
                let _ = prev.send(());
            }
        }
    }
}

fn enqueue_request(
    req: SttRequest,
    live_queue: &mut VecDeque<SttRequest>,
    commit_queue: &mut VecDeque<SttRequest>,
    refine_pending: &mut Option<SttRequest>,
) {
    match req.lane {
        SttLane::Live => enqueue_live_request(req, live_queue),
        SttLane::Commit => commit_queue.push_back(req),
        SttLane::Refine => {
            if let Some(old) = refine_pending.replace(req) {
                let _ = old.result_tx.send(Err(anyhow!(REFINE_SUPERSEDED_ERR)));
            }
        }
    }
}

fn enqueue_live_request(req: SttRequest, live_queue: &mut VecDeque<SttRequest>) {
    let pos = req.utterance_id.and_then(|utterance_id| {
        live_queue
            .iter()
            .rposition(|pending| pending.utterance_id == Some(utterance_id))
    });

    if let Some(pos) = pos {
        let old = std::mem::replace(&mut live_queue[pos], req);
        let _ = old.result_tx.send(Err(anyhow!(LIVE_SUPERSEDED_ERR)));
    } else {
        live_queue.push_back(req);
    }
}

fn pop_next_request(
    live_queue: &mut VecDeque<SttRequest>,
    commit_queue: &mut VecDeque<SttRequest>,
    refine_pending: &mut Option<SttRequest>,
    thermal_level: ThermalLevel,
) -> Option<SttRequest> {
    if let Some(req) = live_queue.pop_front() {
        return Some(req);
    }
    if thermal_level != ThermalLevel::Critical
        && let Some(req) = commit_queue.pop_front()
    {
        return Some(req);
    }
    if matches!(thermal_level, ThermalLevel::Nominal | ThermalLevel::Fair) {
        return refine_pending.take();
    }
    None
}

async fn sleep_for_duty_cycle(
    governor: &SttGovernorConfig,
    thermal_level: ThermalLevel,
    lane: SttLane,
    last_completed_at: Option<Instant>,
) -> Duration {
    let Some(last_completed_at) = last_completed_at else {
        return Duration::ZERO;
    };
    let interval = governor.min_interval_for(lane, thermal_level);
    if interval.is_zero() {
        return Duration::ZERO;
    }
    let next_allowed = last_completed_at + interval;
    let now = Instant::now();
    if next_allowed > now {
        let sleep_for = next_allowed.duration_since(now);
        tokio::time::sleep_until(next_allowed).await;
        return sleep_for;
    }
    Duration::ZERO
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::sync::{Condvar, Mutex as StdMutex};
    use tokio::time::{Duration, timeout};

    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    fn transcript_for_id(id: u32) -> RawTranscript {
        RawTranscript {
            text: format!("job-{id}"),
            segments: Vec::new(),
            ..Default::default()
        }
    }

    fn passthrough_commit_prefilter(samples: &[f32], _sample_rate: u32) -> Vec<f32> {
        samples.to_vec()
    }

    fn assert_duration_near(actual: Duration, expected: Duration) {
        let diff = actual.abs_diff(expected);
        assert!(
            diff <= Duration::from_millis(1),
            "expected {actual:?} to be within 1ms of {expected:?}"
        );
    }

    #[test]
    fn governor_thermal_mapping_keeps_serious_and_critical_throttles() {
        let governor = SttGovernorConfig::test_with_intervals(90, 180, 240);

        assert_eq!(
            governor.min_interval_for(SttLane::Commit, ThermalLevel::Nominal),
            Duration::ZERO,
            "Nominal thermal should not add scheduler duty-cycle latency"
        );
        assert_duration_near(
            governor.min_interval_for(SttLane::Live, ThermalLevel::Fair),
            Duration::from_millis(90),
        );
        assert_duration_near(
            governor.min_interval_for(SttLane::Commit, ThermalLevel::Serious),
            Duration::from_millis(360),
        );
        assert_duration_near(
            governor.min_interval_for(SttLane::Refine, ThermalLevel::Critical),
            Duration::from_millis(960),
        );
    }

    #[tokio::test]
    async fn scheduler_nominal_thermal_skips_duty_cycle_sleep() {
        let governor = SttGovernorConfig::test_with_intervals(200, 200, 200);
        let started = Instant::now();
        let slept = sleep_for_duty_cycle(
            &governor,
            ThermalLevel::Nominal,
            SttLane::Commit,
            Some(Instant::now()),
        )
        .await;

        assert_eq!(slept, Duration::ZERO);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "Nominal thermal should return immediately without sleeping"
        );
    }

    #[test]
    #[serial]
    fn scheduler_initial_prompt_defaults_off_for_all_lanes() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(
            crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
        );
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::remove_var(
                crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
            );
        }

        assert_eq!(initial_prompt_for_lane(SttLane::Live), None);
        assert_eq!(initial_prompt_for_lane(SttLane::Commit), None);
        assert_eq!(initial_prompt_for_lane(SttLane::Refine), None);
    }

    #[tokio::test]
    #[serial]
    async fn scheduler_seeds_prompt_only_for_commit_and_refine_lanes() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(
            crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
        );
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::set_var(
                crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
                "1",
            );
        }

        let captured = Arc::new(StdMutex::new(Vec::<(u32, Option<String>)>::new()));
        let captured_ref = Arc::clone(&captured);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                captured_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((id, initial_prompt));
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(passthrough_commit_prefilter),
        );
        let mut live = scheduler
            .submit(SttLane::Live, vec![1.0], 16_000, None)
            .expect("submit live");
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![2.0], 16_000, None)
            .expect("submit commit");
        let mut refine = scheduler
            .submit(SttLane::Refine, vec![3.0], 16_000, None)
            .expect("submit refine");

        assert_eq!(live.recv().await.expect("live ok").text, "job-1");
        assert_eq!(commit.recv().await.expect("commit ok").text, "job-2");
        assert_eq!(refine.recv().await.expect("refine ok").text, "job-3");
        scheduler.shutdown().await.expect("shutdown");

        let captured = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(captured.len(), 3);
        let live_prompt = captured
            .iter()
            .find(|(id, _)| *id == 1)
            .and_then(|(_, prompt)| prompt.as_ref());
        let commit_prompt = captured
            .iter()
            .find(|(id, _)| *id == 2)
            .and_then(|(_, prompt)| prompt.as_ref());
        let refine_prompt = captured
            .iter()
            .find(|(id, _)| *id == 3)
            .and_then(|(_, prompt)| prompt.as_ref());

        assert!(live_prompt.is_none(), "Live preview must stay unprompted");
        assert!(
            commit_prompt.is_some_and(|prompt| prompt.contains("Loctree")),
            "Commit lane should receive the protected-term prompt"
        );
        assert!(
            refine_prompt.is_some_and(|prompt| prompt.contains("Loctree")),
            "Refine lane should receive the protected-term prompt"
        );
    }

    #[tokio::test]
    async fn scheduler_prioritizes_live_then_commit_then_refine() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let started_ref = Arc::clone(&started);
        let gate_ref = Arc::clone(&gate);

        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);

                if id == 100 {
                    let (lock, cvar) = &*gate_ref;
                    let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*released {
                        released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                    }
                }

                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(passthrough_commit_prefilter),
        );
        let mut block = scheduler
            .submit(SttLane::Live, vec![100.0], 16_000, None)
            .expect("submit blocker");
        let mut refine = scheduler
            .submit(SttLane::Refine, vec![300.0], 16_000, None)
            .expect("submit refine");
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![200.0], 16_000, None)
            .expect("submit commit");
        let mut live = scheduler
            .submit(SttLane::Live, vec![101.0], 16_000, None)
            .expect("submit live");

        {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            cvar.notify_all();
        }

        assert_eq!(block.recv().await.expect("block ok").text, "job-100");
        assert_eq!(live.recv().await.expect("live ok").text, "job-101");
        assert_eq!(commit.recv().await.expect("commit ok").text, "job-200");
        assert_eq!(refine.recv().await.expect("refine ok").text, "job-300");

        scheduler.shutdown().await.expect("shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![100, 101, 200, 300]
        );
    }

    #[tokio::test]
    async fn scheduler_coalesces_pending_refine_requests() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let started_ref = Arc::clone(&started);
        let gate_ref = Arc::clone(&gate);

        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                if id == 1 {
                    let (lock, cvar) = &*gate_ref;
                    let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*released {
                        released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                    }
                }
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_fn(infer);
        let mut block = scheduler
            .submit(SttLane::Live, vec![1.0], 16_000, None)
            .expect("submit block");
        let mut refine_old = scheduler
            .submit(SttLane::Refine, vec![21.0], 16_000, None)
            .expect("submit refine old");
        let mut refine_new = scheduler
            .submit(SttLane::Refine, vec![22.0], 16_000, None)
            .expect("submit refine new");

        {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            cvar.notify_all();
        }

        assert_eq!(block.recv().await.expect("block ok").text, "job-1");
        let old_err = refine_old
            .recv()
            .await
            .expect_err("old refine should be coalesced");
        assert!(
            old_err.to_string().contains("superseded"),
            "unexpected old refine error: {old_err}"
        );
        assert_eq!(
            refine_new.recv().await.expect("new refine ok").text,
            "job-22"
        );

        scheduler.shutdown().await.expect("shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![1, 22]
        );
    }

    #[tokio::test]
    async fn scheduler_coalesces_pending_live_requests_per_utterance() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let gate = Arc::new((StdMutex::new(false), Condvar::new()));
        let started_ref = Arc::clone(&started);
        let gate_ref = Arc::clone(&gate);

        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                if id == 1 {
                    let (lock, cvar) = &*gate_ref;
                    let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*released {
                        released = cvar.wait(released).unwrap_or_else(|e| e.into_inner());
                    }
                }
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_fn(infer);
        let mut block = scheduler
            .submit_for_utterance(SttLane::Live, vec![1.0], 16_000, None, 1)
            .expect("submit block");
        let mut old_live = scheduler
            .submit_for_utterance(SttLane::Live, vec![10.0], 16_000, None, 2)
            .expect("submit old live");
        let mut new_live = scheduler
            .submit_for_utterance(SttLane::Live, vec![11.0], 16_000, None, 2)
            .expect("submit new live");

        {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            cvar.notify_all();
        }

        assert_eq!(block.recv().await.expect("block ok").text, "job-1");
        let old_err = old_live
            .recv()
            .await
            .expect_err("old live should be coalesced");
        assert!(
            old_err.to_string().contains("superseded"),
            "unexpected old live error: {old_err}"
        );
        assert_eq!(new_live.recv().await.expect("new live ok").text, "job-11");

        scheduler.shutdown().await.expect("shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![1, 11]
        );
    }

    #[tokio::test]
    async fn scheduler_enforces_min_interval_between_inferences() {
        let started = Arc::new(StdMutex::new(Vec::<Instant>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Instant::now());
                Ok(transcript_for_id(
                    samples.first().copied().unwrap_or_default() as u32,
                ))
            },
        );

        let scheduler = SttScheduler::with_runtime_fns_and_test_governor(
            infer,
            Arc::new(passthrough_commit_prefilter),
            SttGovernorConfig::test(0, 40),
        );
        scheduler
            .set_thermal_level(ThermalLevel::Fair)
            .expect("set fair thermal");
        let mut live = scheduler
            .submit(SttLane::Live, vec![1.0], 16_000, None)
            .expect("submit live");
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![2.0], 16_000, None)
            .expect("submit commit");

        assert_eq!(live.recv().await.expect("live ok").text, "job-1");
        assert_eq!(commit.recv().await.expect("commit ok").text, "job-2");
        scheduler.shutdown().await.expect("shutdown");

        let times = started.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(times.len(), 2);
        assert!(
            times[1].duration_since(times[0]) >= Duration::from_millis(35),
            "commit inference started too soon after live inference"
        );
    }

    #[tokio::test]
    async fn scheduler_serious_thermal_pauses_refine_lane() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(passthrough_commit_prefilter),
        );
        scheduler
            .set_thermal_level(ThermalLevel::Serious)
            .expect("set serious thermal");
        let mut refine = scheduler
            .submit(SttLane::Refine, vec![3.0], 16_000, None)
            .expect("submit refine");
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![2.0], 16_000, None)
            .expect("submit commit");

        assert_eq!(commit.recv().await.expect("commit ok").text, "job-2");
        let shutdown_task = tokio::spawn(async move { scheduler.shutdown().await });
        let refine_err = refine
            .recv()
            .await
            .expect_err("refine should be cancelled while serious thermal is active");
        assert!(
            refine_err.to_string().contains(SHUTDOWN_ERR),
            "unexpected refine error: {refine_err}"
        );
        shutdown_task
            .await
            .expect("shutdown join")
            .expect("shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![2]
        );
    }

    #[tokio::test]
    async fn scheduler_critical_thermal_runs_only_latest_live() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(passthrough_commit_prefilter),
        );
        scheduler
            .set_thermal_level(ThermalLevel::Critical)
            .expect("set critical thermal");
        let mut commit = scheduler
            .submit_for_utterance(SttLane::Commit, vec![2.0], 16_000, None, 1)
            .expect("submit commit");
        let mut refine = scheduler
            .submit(SttLane::Refine, vec![3.0], 16_000, None)
            .expect("submit refine");
        let mut old_live = scheduler
            .submit_for_utterance(SttLane::Live, vec![10.0], 16_000, None, 2)
            .expect("submit old live");
        let mut new_live = scheduler
            .submit_for_utterance(SttLane::Live, vec![11.0], 16_000, None, 2)
            .expect("submit new live");

        let old_err = old_live
            .recv()
            .await
            .expect_err("old live should be coalesced");
        assert!(
            old_err.to_string().contains("superseded"),
            "unexpected old live error: {old_err}"
        );
        assert_eq!(new_live.recv().await.expect("new live ok").text, "job-11");

        let shutdown_task = tokio::spawn(async move { scheduler.shutdown().await });
        assert!(
            commit
                .recv()
                .await
                .expect_err("commit should remain paused until shutdown")
                .to_string()
                .contains(SHUTDOWN_ERR)
        );
        assert!(
            refine
                .recv()
                .await
                .expect_err("refine should remain paused until shutdown")
                .to_string()
                .contains(SHUTDOWN_ERR)
        );
        shutdown_task
            .await
            .expect("shutdown join")
            .expect("shutdown");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![11]
        );
    }

    #[tokio::test]
    async fn scheduler_commit_returns_empty_when_prefilter_finds_no_speech() {
        let started = Arc::new(StdMutex::new(Vec::<Vec<f32>>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples);
                Ok(RawTranscript {
                    text: "should-not-run".to_string(),
                    segments: Vec::new(),
                    ..Default::default()
                })
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(|_samples, _sample_rate| Vec::new()),
        );
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![0.0, 0.0, 0.0], 16_000, None)
            .expect("submit commit");

        let result = commit
            .recv()
            .await
            .expect("commit should return empty transcript");
        assert!(
            result.text.is_empty(),
            "commit no-speech should return empty text"
        );
        assert!(
            result.segments.is_empty(),
            "commit no-speech should return empty segments"
        );
        assert!(
            started.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "infer should be skipped when commit prefilter returns no speech"
        );

        scheduler.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn scheduler_commit_always_prefilters_short_buffers() {
        let captured = Arc::new(StdMutex::new(Vec::<Vec<f32>>::new()));
        let captured_ref = Arc::clone(&captured);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                captured_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples.clone());
                Ok(RawTranscript {
                    text: format!("len-{}", samples.len()),
                    segments: Vec::new(),
                    ..Default::default()
                })
            },
        );
        let prefilter_inputs = Arc::new(StdMutex::new(Vec::<(Vec<f32>, u32)>::new()));
        let prefilter_inputs_ref = Arc::clone(&prefilter_inputs);

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(move |samples, sample_rate| {
                prefilter_inputs_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((samples.to_vec(), sample_rate));
                vec![42.0]
            }),
        );
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![1.0], 16_000, None)
            .expect("submit commit");

        let result = commit.recv().await.expect("commit should run infer");
        assert_eq!(result.text, "len-1");
        assert_eq!(
            prefilter_inputs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[(vec![1.0], 16_000)],
            "short commit buffers must still flow through commit prefilter"
        );
        assert_eq!(
            captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[vec![42.0]],
            "commit infer should receive prefiltered audio for short buffers too"
        );

        scheduler.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn scheduler_commit_infers_only_prefiltered_speech_audio() {
        let captured = Arc::new(StdMutex::new(Vec::<Vec<f32>>::new()));
        let captured_ref = Arc::clone(&captured);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                captured_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples.clone());
                Ok(RawTranscript {
                    text: format!("len-{}", samples.len()),
                    segments: Vec::new(),
                    ..Default::default()
                })
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(|samples, _sample_rate| {
                samples
                    .iter()
                    .copied()
                    .filter(|sample| *sample > 0.5)
                    .collect()
            }),
        );
        let mut commit = scheduler
            .submit(SttLane::Commit, vec![0.0, 0.8, 0.1, 0.9], 16_000, None)
            .expect("submit commit");

        let result = commit.recv().await.expect("commit should run infer");
        assert_eq!(result.text, "len-2");
        assert!(
            result.segments.is_empty(),
            "mock infer returns no segments for this deterministic contract test"
        );
        assert_eq!(
            captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            &[vec![0.8, 0.9]],
            "commit infer should receive speech-only samples from prefilter"
        );

        scheduler.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn scheduler_shutdown_drains_pending_work() {
        let started = Arc::new(StdMutex::new(Vec::<u32>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>,
                  _initial_prompt: Option<String>|
                  -> Result<RawTranscript> {
                let id = samples.first().copied().unwrap_or_default() as u32;
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id);
                std::thread::sleep(std::time::Duration::from_millis(25));
                Ok(transcript_for_id(id))
            },
        );

        let scheduler = SttScheduler::with_infer_and_commit_prefilter(
            infer,
            Arc::new(passthrough_commit_prefilter),
        );
        let mut first = scheduler
            .submit(SttLane::Live, vec![1.0], 16_000, None)
            .expect("submit first");
        let mut second = scheduler
            .submit(SttLane::Commit, vec![2.0], 16_000, None)
            .expect("submit second");

        let shutdown_task = tokio::spawn(async move { scheduler.shutdown().await });

        assert_eq!(first.recv().await.expect("first ok").text, "job-1");
        assert_eq!(second.recv().await.expect("second ok").text, "job-2");

        timeout(Duration::from_secs(2), shutdown_task)
            .await
            .expect("shutdown timeout")
            .expect("shutdown join")
            .expect("shutdown result");

        assert_eq!(
            started.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            vec![1, 2]
        );
    }
}
