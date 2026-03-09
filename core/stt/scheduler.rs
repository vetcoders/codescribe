use crate::pipeline::contracts::RawTranscript;
use anyhow::{Result, anyhow};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const REFINE_SUPERSEDED_ERR: &str = "STT refine request superseded by a newer pending refine";
const SHUTDOWN_ERR: &str = "STT scheduler is shutting down";

type InferFn = Arc<dyn Fn(Vec<f32>, u32, Option<String>) -> Result<RawTranscript> + Send + Sync>;
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
        Self::with_runtime_fns(infer_fn, Arc::new(default_commit_prefilter))
    }

    #[cfg(test)]
    pub(crate) fn with_infer_and_commit_prefilter(
        infer_fn: InferFn,
        commit_prefilter_fn: CommitPrefilterFn,
    ) -> Self {
        Self::with_runtime_fns(infer_fn, commit_prefilter_fn)
    }

    fn with_runtime_fns(infer_fn: InferFn, commit_prefilter_fn: CommitPrefilterFn) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let worker_handle =
            tokio::spawn(scheduler_worker(command_rx, infer_fn, commit_prefilter_fn));
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
) -> Result<RawTranscript> {
    crate::stt::transcribe_long_with_segments(&samples, sample_rate, language.as_deref())
}

fn default_commit_prefilter(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let (speech, _) = crate::vad::extract_speech(samples, sample_rate);
    speech
}

struct SttRequest {
    lane: SttLane,
    samples: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
    result_tx: oneshot::Sender<Result<RawTranscript>>,
}

enum SchedulerCommand {
    Submit(SttRequest),
    Shutdown { ack_tx: oneshot::Sender<()> },
}

async fn scheduler_worker(
    mut command_rx: mpsc::UnboundedReceiver<SchedulerCommand>,
    infer_fn: InferFn,
    commit_prefilter_fn: CommitPrefilterFn,
) {
    let mut live_queue: VecDeque<SttRequest> = VecDeque::new();
    let mut commit_queue: VecDeque<SttRequest> = VecDeque::new();
    let mut refine_pending: Option<SttRequest> = None;
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
            &mut is_shutting_down,
            &mut shutdown_ack,
        );

        if let Some(req) = pop_next_request(&mut live_queue, &mut commit_queue, &mut refine_pending)
        {
            let infer = Arc::clone(&infer_fn);
            let commit_prefilter = Arc::clone(&commit_prefilter_fn);
            let lane = req.lane;
            let result = tokio::task::spawn_blocking(move || {
                let samples = if lane == SttLane::Commit {
                    // Commit contract (hard): always VAD-prefilter before inference.
                    // Never add env/runtime bypasses that allow raw commit passthrough.
                    // The Commit lane must remain deterministic and VAD-first.
                    let speech = (commit_prefilter)(&req.samples, req.sample_rate);
                    if speech.is_empty() {
                        tracing::info!(
                            "Commit VAD: no speech in {:.1}s utterance — returning empty",
                            req.samples.len() as f32 / req.sample_rate as f32
                        );
                        return Ok(RawTranscript {
                            text: String::new(),
                            segments: Vec::new(),
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
                (infer)(samples, req.sample_rate, req.language)
            })
            .await
            .map_err(|e| anyhow!("STT blocking worker task failed: {}", e))
            .and_then(|r| r);
            let _ = req.result_tx.send(result);
            continue;
        }

        if is_shutting_down {
            break;
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
        SttLane::Live => live_queue.push_back(req),
        SttLane::Commit => commit_queue.push_back(req),
        SttLane::Refine => {
            if let Some(old) = refine_pending.replace(req) {
                let _ = old.result_tx.send(Err(anyhow!(REFINE_SUPERSEDED_ERR)));
            }
        }
    }
}

fn pop_next_request(
    live_queue: &mut VecDeque<SttRequest>,
    commit_queue: &mut VecDeque<SttRequest>,
    refine_pending: &mut Option<SttRequest>,
) -> Option<SttRequest> {
    live_queue
        .pop_front()
        .or_else(|| commit_queue.pop_front())
        .or_else(|| refine_pending.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex as StdMutex};
    use tokio::time::{Duration, timeout};

    fn transcript_for_id(id: u32) -> RawTranscript {
        RawTranscript {
            text: format!("job-{id}"),
            segments: Vec::new(),
        }
    }

    fn passthrough_commit_prefilter(samples: &[f32], _sample_rate: u32) -> Vec<f32> {
        samples.to_vec()
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
                  _language: Option<String>|
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
                  _language: Option<String>|
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
    async fn scheduler_commit_returns_empty_when_prefilter_finds_no_speech() {
        let started = Arc::new(StdMutex::new(Vec::<Vec<f32>>::new()));
        let started_ref = Arc::clone(&started);
        let infer = Arc::new(
            move |samples: Vec<f32>,
                  _sample_rate: u32,
                  _language: Option<String>|
                  -> Result<RawTranscript> {
                started_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples);
                Ok(RawTranscript {
                    text: "should-not-run".to_string(),
                    segments: Vec::new(),
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
                  _language: Option<String>|
                  -> Result<RawTranscript> {
                captured_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples.clone());
                Ok(RawTranscript {
                    text: format!("len-{}", samples.len()),
                    segments: Vec::new(),
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
                  _language: Option<String>|
                  -> Result<RawTranscript> {
                captured_ref
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(samples.clone());
                Ok(RawTranscript {
                    text: format!("len-{}", samples.len()),
                    segments: Vec::new(),
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
                  _language: Option<String>|
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
