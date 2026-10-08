//! Reviewed closed AST language for take transfer and cancellation ownership.
//! Every nested callback, return and await is compared; helper names confer no proof.
use super::Grammar;
use syn::{Block, parse_quote};

pub(super) fn close_capture(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        match self.release_take_pcm_feed() {
            TakeFeedRelease::LastSubscriber | TakeFeedRelease::NoTakeFeed => {
                self.recorder.close_capture().await
            }
            TakeFeedRelease::CaptureShared => false,
        }
    });
    g.require(body == &expected, "BOUNDARY: unsupported release take PCM feed; close physical recorder only for last or absent take, keep shared capture alive");
    if body == &expected {
        g.events.push("release take PCM feed; close physical recorder only for last or absent take, keep shared capture alive".into());
    }
}

pub(super) fn release_take_pcm_feed(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let Some(id) = self.take_subscriber.take() else {
            return TakeFeedRelease::NoTakeFeed;
        };
        if self.release_capture_subscriber(id) {
            TakeFeedRelease::LastSubscriber
        } else {
            TakeFeedRelease::CaptureShared
        }
    });
    g.require(body == &expected, "BOUNDARY: unsupported consume exactly original take subscription; preserve last/shared release result");
    if body == &expected {
        g.events.push(
            "consume exactly original take subscription; preserve last/shared release result"
                .into(),
        );
    }
}

pub(super) fn release_capture_subscriber(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let before = self.capture_subscribers.len();
        self.capture_subscribers.retain(|(held, _)| *held != id);
        if self.capture_subscribers.len() == before {
            warn!(?id, "release of an unknown capture subscriber");
        }
        self.pcm_feeds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|feed| feed.id != id);
        if self.take_subscriber == Some(id) {
            self.take_subscriber = None;
        }
        self.capture_subscribers.is_empty()
    });
    g.require(body == &expected, "BOUNDARY: unsupported remove only matching subscriber and PCM feed under registry lock; return actual remaining ownership");
    if body == &expected {
        g.events.push("remove only matching subscriber and PCM feed under registry lock; return actual remaining ownership".into());
    }
}

pub(super) fn finish_closed_capture(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        if self.terminal_take.is_none() {
            self.terminal_take = Some(self.detach_closed_take(was_active));
        }
        let take = self.terminal_take.as_mut().expect("closed take retained");
        let result = take.finish().await;
        if take.is_settled() {
            self.terminal_take = None;
        }
        result
    });
    g.require(body == &expected, "BOUNDARY: unsupported retain one terminal_take across borrowed finish await; clear only fully settled owner");
    if body == &expected {
        g.events.push(
            "retain one terminal_take across borrowed finish await; clear only fully settled owner"
                .into(),
        );
    }
}

pub(super) fn detach_closed_take(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        if let Some(take) = self.terminal_take.take() {
            return take;
        }
        let archive = self.take_archive.take();
        let physical_archive = if was_active {
            self.recorder.detach_closed_spill()
        } else {
            None
        };
        let stopped = self.prepared_capture_archive.take().or_else(|| {
            archive
                .is_none()
                .then(|| self.recorder.finalize_closed_capture(was_active))
        });
        self.detach_take_state(stopped, archive, physical_archive)
    });
    g.require(body == &expected, "BOUNDARY: unsupported transfer existing terminal owner or exact take archive and optional closed physical spill once");
    if body == &expected {
        g.events.push("transfer existing terminal owner or exact take archive and optional closed physical spill once".into());
    }
}

pub(super) fn detach_take_state(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        ClosedTake {
            stopped,
            archive,
            physical_archive,
            archive_worker: None,
            task_failure: None,
            transcript_buffer: std::mem::replace(
                &mut self.transcript_buffer,
                Arc::new(Mutex::new(String::new())),
            ),
            transcription_handle: self.transcription_handle.take(),
            event_sink: self.event_sink.take(),
            authority_session_id: self.authority_session_id.clone(),
            acoustic_ledger: self.acoustic_ledger.clone(),
            capture_epoch: self.capture_epoch,
            sample_rate: self.sample_rate,
            seal_lane_armed: self.seal_lane_armed(),
            captured_samples: Arc::clone(&self.captured_samples),
            terminal_audio_sender: self.terminal_audio_sender.take(),
            lifecycle_handle: self.lifecycle_handle.take(),
            last_window_closed: self.last_window_closed.take(),
        }
    });
    g.require(body == &expected, "BOUNDARY: unsupported move original task/sink/terminal sender/transcript; retain original ledger, identity, rate and PCM counter");
    if body == &expected {
        g.events.push("move original task/sink/terminal sender/transcript; retain original ledger, identity, rate and PCM counter".into());
    }
}

pub(super) fn is_settled(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        self.archive.is_none()
            && self.physical_archive.is_none()
            && self.archive_worker.is_none()
            && self.transcription_handle.is_none()
            && self.event_sink.is_none()
    });
    g.require(body == &expected, "BOUNDARY: unsupported require both archives, disk worker, transcription handle and publication sink absent before release");
    if body == &expected {
        g.events.push("require both archives, disk worker, transcription handle and publication sink absent before release".into());
    }
}

pub(super) fn finish(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        if self.archive_worker.is_none()
            && (self.archive.is_some() || self.physical_archive.is_some())
        {
            let archive = self.archive.take();
            let physical_archive = self.physical_archive.take();
            let expected_samples = self.captured_samples.load(Ordering::Relaxed);
            let supplied = self.stopped.take();
            self.archive_worker = Some(tokio::task::spawn_blocking(move || {
                let stopped = match archive {
                    Some(archive) => finalize_take_archive(archive, expected_samples),
                    None => supplied.unwrap_or(Ok(None)),
                };
                if let Some(archive) = physical_archive
                    && let Err(error) = archive.finalize()
                {
                    match error.recover_complete_archive() {
                        Ok(recovered) => {
                            error.acknowledge_recovery(&recovered)?;
                            warn!(source = ?error.source_path, recovered = %recovered.path().display(),
                                "auxiliary physical archive recovered separately from owned take");
                        }
                        Err(recovery) => {
                            let audio_path = stopped.as_ref().ok().and_then(Clone::clone);
                            let cause = stopped.err().unwrap_or_else(|| anyhow!(
                                "auxiliary physical archive recovery failed: {recovery:#}; exact take WAV remains separate"
                            ));
                            return Err(anyhow::Error::new(CaptureStopFailure {
                                session_id: None,
                                capture_epoch: 0,
                                audio_path,
                                cause,
                                task_failure: Some(anyhow::Error::new(error)),
                            }));
                        }
                    }
                }
                stopped
            }));
        }
        if let Some(worker) = self.archive_worker.as_mut() {
            self.stopped = Some(match worker.await {
                Ok(stopped) => stopped,
                Err(error) => Err(anyhow!("take archive worker failed: {error}")),
            });
            self.archive_worker = None;
        }
        let stopped = self.stopped.get_or_insert(Ok(None));
        if let Some(sender) = self.terminal_audio_sender.take() {
            let owned_path = stopped
                .as_ref()
                .ok()
                .and_then(|path| path.as_ref())
                .or_else(|| {
                    stopped
                        .as_ref()
                        .err()
                        .and_then(|error| error.downcast_ref::<CaptureStopFailure>())
                        .and_then(|failure| failure.audio_path.as_ref())
                });
            let receipt = match (owned_path, &*stopped) {
                (Some(path), _) => Ok(
                    crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                        session_id: self.authority_session_id.clone().unwrap_or_default(),
                        capture_epoch: self.capture_epoch,
                        sample_rate: self.sample_rate,
                        sample_count: self.captured_samples.load(Ordering::Relaxed),
                        path: path.clone(),
                    },
                ),
                (None, Ok(_)) => Err("capture finalized without a WAV archive".into()),
                (None, Err(error)) => Err(format!("capture archive finalization failed: {error}")),
            };
            let _ = sender.send(receipt);
        }
        self.lifecycle_handle = None;

        if let Some(handle) = self.transcription_handle.as_mut() {
            debug!("Waiting for transcription session task to finish...");
            self.task_failure = handle
                .await
                .context("Transcription session task failed")
                .err();
            self.transcription_handle = None;
        }

        let drain_failure = match self.event_sink.as_ref() {
            Some(sink) => match self.authority_session_id.as_deref() {
                Some(session_id) => tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    sink.wait_presentation_published(session_id, self.capture_epoch),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow!("presentation terminal drain timed out")))
                .err(),
                None => Some(anyhow!(
                    "presentation terminal drain has no capture identity"
                )),
            },
            None => None,
        };
        if drain_failure.is_none() {
            self.event_sink = None;
        }

        let transcript = self.transcript_buffer.lock().await.clone();
        if let Some(drain) = drain_failure {
            let stopped = self.stopped.as_ref().expect("archive result retained");
            let task_failure = self.task_failure.as_ref().map(copy_stop_error);
            let (audio_path, cause, secondary) = match stopped {
                Ok(path) => (path.clone(), task_failure, None),
                Err(error) => match error.downcast_ref::<CaptureStopFailure>() {
                    Some(failure) => {
                        let secondary = match (
                            failure.task_failure.as_ref().map(copy_stop_error),
                            task_failure,
                        ) {
                            (Some(archive), Some(task)) => Some(
                                archive
                                    .context(format!("transcription task also failed: {task:#}")),
                            ),
                            (archive, task) => archive.or(task),
                        };
                        (
                            failure.audio_path.clone(),
                            Some(copy_stop_error(&failure.cause)),
                            secondary,
                        )
                    }
                    None => (None, Some(copy_stop_error(error)), task_failure),
                },
            };
            let cause = match cause {
                Some(cause) => cause.context(format!("presentation drain also failed: {drain:#}")),
                None => drain,
            };
            return Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause,
                task_failure: secondary,
            }));
        }

        let stopped = self.stopped.take().expect("archive result retained");
        let task_failure = self.task_failure.take();
        let (audio_path, cause, task_failure) = match stopped {
            Ok(path) => (path, task_failure, None),
            Err(error) => match error.downcast::<CaptureStopFailure>() {
                Ok(failure) => {
                    let secondary = match (failure.task_failure, task_failure) {
                        (Some(archive), Some(task)) => Some(
                            archive.context(format!("transcription task also failed: {task:#}")),
                        ),
                        (archive, task) => archive.or(task),
                    };
                    (failure.audio_path, Some(failure.cause), secondary)
                }
                Err(error) => (None, Some(error), task_failure),
            },
        };
        if let Some(cause) = cause {
            return Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause,
                task_failure,
            }));
        }

        let captured_samples = self.captured_samples.load(Ordering::Relaxed);
        let empty_capture = captured_samples <= u64::from(self.sample_rate) * 3 / 10
            && transcript.is_empty()
            && self.acoustic_ledger.as_ref().is_none_or(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .has_no_capture_facts()
            });
        if empty_capture {
            return Ok((transcript, audio_path));
        }
        let finality = self.authority_session_id.as_deref().and_then(|session| {
            self.acoustic_ledger.as_ref().map(|ledger| {
                ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .terminal_finality(session, self.capture_epoch)
            })
        });
        match finality {
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Sealed(_)) => {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::ObservedSilence(_))
                if transcript.is_empty() =>
            {
                Ok((transcript, audio_path))
            }
            Some(crate::pipeline::acoustic_ledger::TerminalFinality::Refused(finality)) => {
                Err(anyhow::Error::new(TerminalSealRefused {
                    finality,
                    audio_path,
                    committed_text: transcript,
                }))
            }
            _ => Err(anyhow::Error::new(CaptureStopFailure {
                session_id: self.authority_session_id.clone(),
                capture_epoch: self.capture_epoch,
                audio_path,
                cause: anyhow!("recording terminal authority unavailable or inconsistent"),
                task_failure: None,
            })),
        }
    });
    g.require(body == &expected, "BOUNDARY: unsupported retain blocking archive worker before await; borrow producer join and sink; preserve retry outcomes; inspect ledger finality only after publication and transcript awaits");
    if body == &expected {
        g.events.push("retain blocking archive worker before await; borrow producer join and sink; preserve retry outcomes; inspect ledger finality only after publication and transcript awaits".into());
    }
}

pub(super) fn copy_stop_error(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        if let Some(archive) = error.downcast_ref::<crate::audio::recorder::CaptureArchiveError>() {
            return anyhow::Error::new(archive.clone());
        }
        if let Some(failure) = error.downcast_ref::<CaptureStopFailure>() {
            return anyhow::Error::new(CaptureStopFailure {
                session_id: failure.session_id.clone(),
                capture_epoch: failure.capture_epoch,
                audio_path: failure.audio_path.clone(),
                cause: copy_stop_error(&failure.cause),
                task_failure: failure.task_failure.as_ref().map(copy_stop_error),
            });
        }
        anyhow!("{error:#}")
    });
    g.require(body == &expected, "BOUNDARY: unsupported clone diagnostic with original archive recovery Arcs and identity; never manufacture or replace owned PCM");
    if body == &expected {
        g.events.push("clone diagnostic with original archive recovery Arcs and identity; never manufacture or replace owned PCM".into());
    }
}

pub(super) fn finalize_take_archive(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        let (path, count) = archive.finalize().map_err(|mut error| {
            error.captured_samples = usize::try_from(expected_samples).unwrap_or(usize::MAX);
            anyhow::Error::new(error)
        })?;
        anyhow::ensure!(
            count as u64 == expected_samples,
            "take archive sample count disagrees: captured={expected_samples}, written={count}; source={}",
            path.display(),
        );
        Ok(Some(path))
    });
    g.require(body == &expected, "BOUNDARY: unsupported require exact archived sample count and preserve explicit typed archive refusal");
    if body == &expected {
        g.events.push(
            "require exact archived sample count and preserve explicit typed archive refusal"
                .into(),
        );
    }
}
