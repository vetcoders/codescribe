//! Production-owned replay seam for private overlay quality evaluation.
//!
//! Audio ingress is the only substituted boundary: decoded fixture PCM is fed
//! in 100 ms chunks instead of arriving from CoreAudio. Everything downstream
//! is shared with the overlay: recording-start Layer 1 policy, `SessionConfig`,
//! `transcription_session`, the session-owned `AcousticLedger`, and the
//! `TranscriptReducer` projection delivered to the overlay.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use codescribe_core::audio::streaming_recorder::{
    CaptureTurnIntent, RecordedSessionReplay, ReplayInputReceipt, ReplayScenario,
    prepare_recorded_replay, replay_production_session,
};
use codescribe_core::config::UserSettings;
use codescribe_core::pipeline::acoustic_ledger::AcousticLedger;
use codescribe_core::pipeline::contracts::{
    EngineEvent, EventSink, FinalPassDisposition, TranscriptionVerdict,
};
use codescribe_core::pipeline::replay_card::{ReplayBuild, ReplayCard};
use codescribe_core::pipeline::streaming::{
    APPLE_FINAL_OVERLAP_WARNING_CODE, replay_buffered_engine_session, retained_formatter_deadline,
};

use crate::presentation::emitter::{PresentationEmitter, TranscriptReducer};
use crate::presentation::transcript_bus::{
    HISTORY_SCHEMA, TranscriptBus, TranscriptMode, TranscriptSession,
};
use crate::presentation::transcript_projection::EVIDENCE_SCHEMA;

/// Which production stop lane a corpus replay should exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionReplayLane {
    /// Production ledger projection without an additional full-file observation.
    AppleLexicon,
    /// Production ledger projection plus a diagnostic local full-file observation.
    LocalFinalPass,
}

impl ProductionReplayLane {
    /// Stable content-free token for reports and filenames.
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::AppleLexicon => "apple_lexicon",
            Self::LocalFinalPass => "local_final_pass",
        }
    }
}

/// In-memory result used to calculate content-redacting quality metrics.
///
/// Transcript bodies intentionally have no serialization implementation. The
/// corpus runner must reduce them to counts/scores before writing artifacts.
#[derive(Debug)]
pub struct ProductionOverlayReplay {
    pub lane: ProductionReplayLane,
    pub events: Vec<EngineEvent>,
    pub live_text: String,
    pub adjudicated_text: String,
    pub delivered_text: String,
    pub layer1_armed: bool,
    pub transcript_source: Option<String>,
    pub engine_label: Option<String>,
    pub final_pass_attempted: bool,
    pub final_pass_skipped: bool,
    pub final_pass_skip_reason: Option<String>,
    pub boundary_evidence: ReplayBoundaryEvidence,
}

/// Content-free final-boundary evidence emitted for every replay recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBoundaryEvidence {
    pub final_count: usize,
    pub unique_final_id_count: usize,
    pub repeated_final_id_count: usize,
    pub overlapping_final_window_count: usize,
}

fn boundary_evidence(events: &[EngineEvent]) -> ReplayBoundaryEvidence {
    let mut ids = HashSet::new();
    let mut windows = Vec::<(f32, f32)>::new();
    let mut final_count = 0usize;
    let mut repeated_final_id_count = 0usize;
    let mut overlapping_final_window_count = 0usize;

    for event in events {
        if matches!(event, EngineEvent::Warning { code, .. } if code == APPLE_FINAL_OVERLAP_WARNING_CODE)
        {
            overlapping_final_window_count += 1;
            continue;
        }
        let EngineEvent::UtteranceFinal {
            utterance_id,
            start_ts,
            end_ts,
            ..
        } = event
        else {
            continue;
        };
        final_count += 1;
        if !ids.insert(*utterance_id) {
            repeated_final_id_count += 1;
        }
        if start_ts.is_finite() && end_ts.is_finite() && end_ts > start_ts {
            if windows
                .iter()
                .any(|(prior_start, prior_end)| start_ts < prior_end && end_ts > prior_start)
            {
                overlapping_final_window_count += 1;
            }
            windows.push((*start_ts, *end_ts));
        }
    }

    ReplayBoundaryEvidence {
        final_count,
        unique_final_id_count: ids.len(),
        repeated_final_id_count,
        overlapping_final_window_count,
    }
}

struct ReplayDelivery {
    adjudicated_text: String,
    delivered_text: String,
    transcript_source: Option<String>,
    engine_label: Option<String>,
    final_pass_attempted: bool,
    final_pass_skipped: bool,
    final_pass_skip_reason: Option<String>,
}

/// Project authenticated ledger events through the reducer of record.
fn project_ledger_truth(events: &[EngineEvent], ledger: &mut AcousticLedger) -> Result<String> {
    let mut reducer = TranscriptReducer::default();
    let mut rendered = None;
    for event in events {
        let revision = match event {
            EngineEvent::LedgerMutation {
                observation,
                receipt,
                ..
            } => reducer.apply_ledger_mutation(ledger, observation, receipt),
            EngineEvent::LedgerSeal { receipt } => reducer.apply_ledger_seal(receipt),
            EngineEvent::OccurrenceLabelProposal { proposal } => {
                // A formatter result is a derived version of sealed text; it
                // never revises the Raw document this projection tracks.
                let _ = reducer.apply_occurrence_label_proposal(ledger, proposal);
                None
            }
            _ => None,
        };
        if let Some(revision) = revision {
            rendered = Some(revision.rendered_text);
        }
    }
    rendered
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("production ledger projection produced no deliverable text"))
}

/// Delivery is a projection of the ledger, never a second text adjudication.
/// An optional full-file pass remains diagnostic evidence for corpus reports;
/// it cannot replace, append, or post-process the projected occurrences.
fn finish_replay_delivery(
    live_text: String,
    local_final_pass_verdict: Option<TranscriptionVerdict>,
    streaming_engine_label: &str,
) -> Result<ReplayDelivery> {
    if live_text.trim().is_empty() {
        return Err(anyhow!(
            "production ledger projection produced no deliverable text"
        ));
    }
    let final_pass_attempted = local_final_pass_verdict.is_some();
    let (final_pass_skipped, final_pass_skip_reason) = local_final_pass_verdict
        .as_ref()
        .and_then(|verdict| verdict.final_pass.as_ref())
        .map(|pass| {
            (
                matches!(
                    pass.disposition,
                    FinalPassDisposition::Skipped
                        | FinalPassDisposition::Rejected
                        | FinalPassDisposition::Dropped
                ),
                pass.reason.clone(),
            )
        })
        .unwrap_or_else(|| {
            (
                !final_pass_attempted,
                (!final_pass_attempted).then(|| "not_attempted".to_string()),
            )
        });
    Ok(ReplayDelivery {
        adjudicated_text: live_text.clone(),
        delivered_text: live_text,
        transcript_source: Some("ledger_projection".to_string()),
        engine_label: Some(streaming_engine_label.to_string()),
        final_pass_attempted,
        final_pass_skipped,
        final_pass_skip_reason,
    })
}

/// Replay one WAV through the production overlay engine cone.
pub async fn replay_overlay_recording(
    wav: &Path,
    language: Option<String>,
    settings: &UserSettings,
    lane: ProductionReplayLane,
) -> Result<ProductionOverlayReplay> {
    let wav = wav.to_path_buf();
    let settings = settings.clone();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        let (wav, _audio_lease) =
            codescribe_core::state::history::audio_retention::AudioReadLease::acquire_for_path(
                &codescribe_core::config::Config::config_dir(),
                &wav,
            )
            .map_err(|_| anyhow!("acquire replay audio input lease failed"))?;
        // Own the guard across both the initial decode and the later file final
        // pass, even if the public future is cancelled while replay is pending.
        runtime.block_on(replay_overlay_recording_on_worker(
            &wav, language, &settings, lane,
        ))
    })
    .await
    .map_err(|_| anyhow!("replay audio worker join failed"))?
}

async fn replay_overlay_recording_on_worker(
    wav: &Path,
    language: Option<String>,
    settings: &UserSettings,
    lane: ProductionReplayLane,
) -> Result<ProductionOverlayReplay> {
    let (samples, sample_rate) = codescribe_core::audio::load_audio_file(wav)
        .map_err(|_| anyhow!("load replay audio failed"))?;
    if samples.is_empty() {
        return Err(anyhow!("replay WAV contains no samples"));
    }

    let session =
        replay_production_session(&samples, sample_rate, language.clone(), settings).await?;
    let live_text = {
        let mut ledger = session
            .acoustic_ledger
            .lock()
            .map_err(|_| anyhow!("production ledger lock poisoned"))?;
        project_ledger_truth(&session.events, &mut ledger)?
    };

    let final_verdict = match lane {
        ProductionReplayLane::AppleLexicon => None,
        ProductionReplayLane::LocalFinalPass => Some(
            codescribe_core::stt::transcribe_file_verdict(wav, language.as_deref())
                .map_err(|_| anyhow!("production local final pass failed"))?,
        ),
    };
    let delivery = finish_replay_delivery(
        live_text.clone(),
        final_verdict,
        &session.streaming_engine_label,
    )?;

    let boundary_evidence = boundary_evidence(&session.events);
    Ok(ProductionOverlayReplay {
        lane,
        events: session.events,
        live_text,
        adjudicated_text: delivery.adjudicated_text,
        delivered_text: delivery.delivered_text,
        layer1_armed: session.layer1_armed,
        transcript_source: delivery.transcript_source,
        engine_label: delivery.engine_label,
        final_pass_attempted: delivery.final_pass_attempted,
        final_pass_skipped: delivery.final_pass_skipped,
        final_pass_skip_reason: delivery.final_pass_skip_reason,
        boundary_evidence,
    })
}

/// One run of a recorded take through the overlay's presentation chain.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordedTakeRun {
    pub run: u32,
    pub session_id: String,
    /// The last Raw document the transcript bus accepted for this run: the
    /// row `stt` reads as the overlay block for a live take.
    pub text: String,
    pub reducer_action: Option<String>,
    pub reducer_revision: Option<u64>,
    /// Document rows the bus accepted during the run.
    pub document_rows: usize,
    pub elapsed_seconds: f64,
    /// Engine warning codes, without their messages.
    pub warning_codes: BTreeSet<String>,
    /// `replays/<session>/<measurement>/` in the data dir: the run's private bus and the
    /// sidecars its session wrote (trail, slots, replay card), comparable
    /// with the recorded take's own.
    pub evidence_dir: PathBuf,
}

/// A recorded take replayed once, with the inputs it resolved.
pub struct RecordedTakeReplay {
    pub inputs: Vec<ReplayInputReceipt>,
    pub recorded_build: Option<ReplayBuild>,
    pub run: RecordedTakeRun,
}

/// The take sink's routing (`TakeExternalEventSink`): every event and the
/// completion protocol go to the one presentation owner.
struct RecordedTakeSink {
    presentation: PresentationEmitter,
    warning_codes: std::sync::Mutex<BTreeSet<String>>,
}

impl EventSink for RecordedTakeSink {
    fn consultation_destinations(&self) -> usize {
        self.presentation.consultation_destinations()
    }

    fn on_consultation_completed(
        &self,
        completed: &codescribe_core::agent::consultation::ConsultationGroupAnswer,
    ) -> Result<()> {
        self.presentation.on_consultation_completed(completed)
    }

    fn on_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        self.presentation
            .on_capture_opened(session_id, capture_epoch);
    }

    fn wait_presentation_published<'a>(
        &'a self,
        session_id: &'a str,
        capture_epoch: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        self.presentation
            .wait_presentation_published(session_id, capture_epoch)
    }

    fn on_event(&self, event: &EngineEvent) {
        self.presentation.on_event(event);
        if let EngineEvent::Warning { code, .. } = event {
            self.warning_codes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(code.clone());
        }
    }
}

/// Replay one recorded take the way the overlay saw it.
///
/// The session comes from the take's replay card (see
/// [`prepare_recorded_replay`]). Its events reach the presentation emitter the
/// live take or channel builds from the same snapshot, publishing to a private
/// transcript bus: the bus authenticates every revision before the emitter
/// paints it, so a replay without one would show text the overlay refused.
/// The run keeps its evidence in `replays/<session>/`, never in `sessions/`.
pub async fn replay_recorded_take(
    recorded_session: Option<String>,
    card: Option<ReplayCard>,
    wav: &Path,
    run: u32,
    scenario: ReplayScenario,
) -> Result<RecordedTakeReplay> {
    let wav = wav.to_path_buf();
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        let (wav, _audio_lease) =
            codescribe_core::state::history::audio_retention::AudioReadLease::acquire_for_path(
                &codescribe_core::config::Config::config_dir(),
                &wav,
            )
            .map_err(|_| anyhow!("acquire replay audio input lease failed"))?;
        runtime.block_on(replay_recorded_take_on_worker(
            recorded_session.as_deref(),
            card.as_ref(),
            &wav,
            run,
            &scenario,
        ))
    })
    .await
    .map_err(|_| anyhow!("replay audio worker join failed"))?
}

async fn replay_recorded_take_on_worker(
    recorded_session: Option<&str>,
    card: Option<&ReplayCard>,
    wav: &Path,
    run: u32,
    scenario: &ReplayScenario,
) -> Result<RecordedTakeReplay> {
    let prepared = prepare_recorded_replay(recorded_session, card, wav, run, scenario)?;
    let RecordedSessionReplay {
        samples,
        config,
        inputs,
        recorded_build,
    } = prepared;
    let session_id = config.session_id.clone();
    let capture_epoch = config.capture_epoch;
    let channel = config.capture_turn == CaptureTurnIntent::AgentChannel;
    let settings = Arc::clone(&config.runtime_settings);
    let ledger = Arc::clone(&config.acoustic_ledger);

    // The session reads the live data dir (custom lexicon, protected terms,
    // models), so it runs there. The evidence, not the session, is moved.
    let root = codescribe_core::config::Config::config_dir();
    let evidence = ReplayEvidence::open(&root, &session_id)
        .map_err(|error| anyhow!("prepare replay evidence failed: {}", error.kind()))?;
    let bus_path = evidence.dir.join("transcript-events.jsonl");
    let bus = Arc::new(
        TranscriptBus::open_at(
            TranscriptSession {
                session_id: session_id.clone(),
                mode: if channel {
                    TranscriptMode::Agent
                } else {
                    TranscriptMode::Dictation
                },
                has_latched_target: false,
                latched_target_is_self: false,
                audience: channel.then(|| "replay".to_string()),
                badge_only: channel,
            },
            bus_path.clone(),
            None,
        )
        .map_err(|_| anyhow!("open the replay transcript bus failed"))?,
    );
    bus.publish_started();
    // The live constructors: `channel_presentation_emitter` for a channel, and
    // the take's `RecordingEventPipeline` knobs, minus its UI observers.
    let presentation = if channel {
        super::agent_channel::channel_presentation_emitter(
            Arc::clone(&bus),
            Arc::clone(&ledger),
            &settings,
        )
    } else {
        PresentationEmitter::new_with_authority(
            Arc::new(tokio::sync::Mutex::new(String::new())),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        )
        .with_sentence_pause_sec(settings.values().light_plus_sentence_pause_sec)
        .with_transcription_controls(
            settings.seal_ledger_enabled(),
            settings.apple_live_enabled(),
        )
    };
    let mut sink = Arc::new(RecordedTakeSink {
        presentation,
        warning_codes: std::sync::Mutex::new(BTreeSet::new()),
    });
    sink.on_capture_opened(&session_id, capture_epoch);

    let started = std::time::Instant::now();
    let replayed = replay_buffered_engine_session(&samples, config, sink.clone()).await;
    let elapsed_seconds = started.elapsed().as_secs_f64();
    // Formatter jobs the take accepted outlive its acoustic session on their
    // own owner, which holds the sink until they settle or its deadline drops
    // them. The live emitter keeps receiving those results, so the document
    // is finished only after that owner lets go.
    let owner_deadline = std::time::Instant::now() + retained_formatter_deadline(&settings);
    let finished = loop {
        if let Some(sink) = Arc::get_mut(&mut sink) {
            sink.presentation.finish().await;
            break Ok(());
        }
        if std::time::Instant::now() >= owner_deadline {
            break Err(anyhow!(
                "replay sink still shared after the retained formatter deadline"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    drop(bus);
    let rows = std::fs::read_to_string(&bus_path);
    let moved = evidence.collect_session_sidecars(&root, &session_id);
    replayed?;
    moved.map_err(|error| anyhow!("collect replay evidence failed: {}", error.kind()))?;
    finished?;
    let rows = rows.map_err(|_| anyhow!("read the replay transcript bus failed"))?;

    let documents: Vec<serde_json::Value> = rows
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|row| {
            row["session_id"] == session_id.as_str()
                && matches!(
                    row["schema"].as_str(),
                    Some(EVIDENCE_SCHEMA | HISTORY_SCHEMA)
                )
                && row["rendered_text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
        })
        .collect();
    let last = documents.last();
    // Reuse the live truth observer: the Lab reads these exact ledger pins,
    // never reconstructs final words from decoder candidates or trail text.
    let truth = codescribe_core::pipeline::take_truth::TakeTruth::observe_ledger(
        &ledger.lock().unwrap_or_else(|error| error.into_inner()),
        &session_id,
        capture_epoch,
    );
    codescribe_core::pipeline::take_truth::write_truth_sidecar(
        &evidence.dir.join(format!("{session_id}.wav")),
        &truth,
    )?;
    let warning_codes = std::mem::take(
        &mut *sink
            .warning_codes
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
    );
    Ok(RecordedTakeReplay {
        inputs,
        recorded_build,
        run: RecordedTakeRun {
            run,
            session_id,
            text: last
                .and_then(|row| row["rendered_text"].as_str())
                .unwrap_or_default()
                .to_string(),
            reducer_action: last
                .and_then(|row| row["reducer_action"].as_str())
                .map(str::to_string),
            reducer_revision: last.and_then(|row| row["reducer_revision"].as_u64()),
            document_rows: documents.len(),
            elapsed_seconds,
            warning_codes,
            evidence_dir: evidence.dir.clone(),
        },
    })
}

/// A replay run's evidence directory, held under an exclusive lock.
///
/// The replay session id is stable per take and run, so two replays of one
/// take queue on the lock instead of sharing a bus or a trail. Each run
/// starts from nothing: leftovers of an interrupted run would otherwise
/// trip the trail's single start record.
struct ReplayEvidence {
    dir: PathBuf,
    root: PathBuf,
    session_id: String,
    _lock: std::fs::File,
}

impl ReplayEvidence {
    fn open(root: &Path, session_id: &str) -> std::io::Result<Self> {
        let replays = root.join("replays");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&replays)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(replays.join(format!("{session_id}.lock")))?;
        lock.lock()?;
        // Repeating a measurement must preserve earlier builds and scenarios.
        let dir = replays
            .join(session_id)
            .join(uuid::Uuid::new_v4().to_string());
        let leftovers = session_sidecars(root, session_id)?;
        if !leftovers.is_empty() {
            let interrupted = replays
                .join(session_id)
                .join(format!("interrupted-{}", uuid::Uuid::new_v4()));
            builder.create(&interrupted)?;
            for leftover in leftovers {
                if let Some(name) = leftover.file_name() {
                    std::fs::rename(&leftover, interrupted.join(name))?;
                }
            }
        }
        builder.create(&dir)?;
        Ok(Self {
            dir,
            root: root.to_path_buf(),
            session_id: session_id.to_string(),
            _lock: lock,
        })
    }

    /// Move what the session wrote under `sessions/` into the evidence dir.
    fn collect_session_sidecars(&self, root: &Path, session_id: &str) -> std::io::Result<()> {
        for sidecar in session_sidecars(root, session_id)? {
            if let Some(name) = sidecar.file_name() {
                std::fs::rename(&sidecar, self.dir.join(name))?;
            }
        }
        Ok(())
    }
}

impl Drop for ReplayEvidence {
    fn drop(&mut self) {
        // The session can fail before the success path collects its sidecars.
        // Keep those diagnostics under the same lock instead of deleting them on retry.
        if let Err(error) = self.collect_session_sidecars(&self.root, &self.session_id) {
            tracing::warn!(evidence_dir = %self.dir.display(), %error, "replay evidence collection failed");
        }
    }
}

/// `sessions/<session>.*`. The session alphabet has no dot, so the prefix
/// with its dot names exactly this session's files.
fn session_sidecars(root: &Path, session_id: &str) -> std::io::Result<Vec<PathBuf>> {
    let prefix = format!("{session_id}.");
    let entries = match std::fs::read_dir(root.join("sessions")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut sidecars = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        {
            sidecars.push(entry.path());
        }
    }
    Ok(sidecars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codescribe_core::pipeline::contracts::TranscriptSegment;

    #[test]
    fn replay_evidence_takes_only_its_own_session_files() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let session = "replay-agent-channel-3-0123456789-r1";
        let stale = root.path().join("replays").join(session);
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("transcript-events.jsonl"), "old run\n").unwrap();
        std::fs::write(sessions.join(format!("{session}.trail.jsonl")), "left over").unwrap();

        let evidence = ReplayEvidence::open(root.path(), session).unwrap();
        assert!(
            std::fs::read_dir(&evidence.dir).unwrap().next().is_none(),
            "a run starts from nothing"
        );
        assert!(!sessions.join(format!("{session}.trail.jsonl")).exists());
        assert_eq!(
            std::fs::read_to_string(stale.join("transcript-events.jsonl")).unwrap(),
            "old run\n"
        );

        for name in [
            format!("{session}.trail.jsonl"),
            format!("{session}.slots.jsonl"),
            "replay-agent-channel-3-0123456789-r10.trail.jsonl".to_string(),
            "agent-channel-3-0123456789.trail.jsonl".to_string(),
        ] {
            std::fs::write(sessions.join(name), "row\n").unwrap();
        }
        evidence
            .collect_session_sidecars(root.path(), session)
            .unwrap();

        let mut collected: Vec<_> = std::fs::read_dir(&evidence.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        collected.sort();
        assert_eq!(
            collected,
            [
                format!("{session}.slots.jsonl"),
                format!("{session}.trail.jsonl")
            ]
        );
        assert!(
            sessions
                .join("agent-channel-3-0123456789.trail.jsonl")
                .exists()
        );
        assert!(
            sessions
                .join("replay-agent-channel-3-0123456789-r10.trail.jsonl")
                .exists()
        );
    }

    #[test]
    fn failed_replay_guard_preserves_partial_sidecars() {
        let root = tempfile::tempdir().unwrap();
        let sessions = root.path().join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        let evidence = ReplayEvidence::open(root.path(), "replay-take-r1").unwrap();
        let directory = evidence.dir.clone();
        std::fs::write(
            sessions.join("replay-take-r1.trail.jsonl"),
            "partial diagnostic",
        )
        .unwrap();
        drop(evidence);
        let next = ReplayEvidence::open(root.path(), "replay-take-r1").unwrap();
        assert_ne!(directory, next.dir);
        assert_eq!(
            std::fs::read_to_string(directory.join("replay-take-r1.trail.jsonl")).unwrap(),
            "partial diagnostic"
        );
    }

    #[test]
    fn repeating_a_replay_preserves_previous_measurements() {
        let root = tempfile::tempdir().unwrap();
        let first = ReplayEvidence::open(root.path(), "replay-take-r1").unwrap();
        let previous = first.dir.clone();
        std::fs::write(previous.join("replay-receipt.json"), "first build").unwrap();
        drop(first);
        let second = ReplayEvidence::open(root.path(), "replay-take-r1").unwrap();
        assert_ne!(previous, second.dir);
        assert_eq!(
            std::fs::read_to_string(previous.join("replay-receipt.json")).unwrap(),
            "first build"
        );
    }

    fn final_event(id: u64, text: &str, start_ts: f32, end_ts: f32) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: id,
            text: text.to_string(),
            raw_text: text.to_string(),
            start_ts,
            end_ts,
            segments: vec![TranscriptSegment {
                text: text.to_string(),
                start_ts,
                end_ts,
                confidence: None,
            }],
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        }
    }

    #[test]
    fn replay_delivery_preserves_the_ledger_projection_without_rewrite() {
        let delivery = finish_replay_delivery(
            "Uzywam doker do kontenerow.".to_string(),
            None,
            "live_apple",
        )
        .expect("ledger projection should remain deliverable");
        assert_eq!(
            delivery.transcript_source.as_deref(),
            Some("ledger_projection")
        );
        assert_eq!(delivery.delivered_text, "Uzywam doker do kontenerow.");
        assert_eq!(delivery.adjudicated_text, delivery.delivered_text);
        assert_eq!(delivery.engine_label.as_deref(), Some("live_apple"));
        assert!(!delivery.final_pass_attempted);
        assert!(delivery.final_pass_skipped);
        assert_eq!(
            delivery.final_pass_skip_reason.as_deref(),
            Some("not_attempted")
        );
    }

    /// The exact field consumed by the production replay JSON must name the
    /// live Apple canvas when Layer 1 and local final pass are both disarmed.
    #[test]
    fn apple_only_replay_json_surface_never_reports_streaming_whisper() {
        let delivery = finish_replay_delivery("pacjent stabilny".to_string(), None, "live_apple")
            .expect("Apple live floor should remain deliverable");

        let row = serde_json::json!({ "engine_label": delivery.engine_label });
        assert_eq!(row["engine_label"], "live_apple");
        assert_ne!(row["engine_label"], "streaming_whisper");
    }

    #[test]
    fn boundary_evidence_is_content_free_and_counts_id_and_window_conflicts() {
        let events = vec![
            final_event(1, "one", 0.0, 1.0),
            final_event(1, "revision", 0.0, 1.0),
            final_event(2, "two", 0.5, 2.0),
            final_event(3, "three", 2.0, 3.0),
        ];
        assert_eq!(
            boundary_evidence(&events),
            ReplayBoundaryEvidence {
                final_count: 4,
                unique_final_id_count: 3,
                repeated_final_id_count: 1,
                overlapping_final_window_count: 2,
            }
        );
    }

    #[tokio::test]
    async fn replay_load_failure_never_discloses_private_path() {
        let basename = format!("private-corpus-{}-must-not-leak.wav", std::process::id());
        let path = std::env::temp_dir().join(&basename);
        let error = replay_overlay_recording(
            &path,
            Some("pl".to_string()),
            &UserSettings::default(),
            ProductionReplayLane::AppleLexicon,
        )
        .await
        .expect_err("a missing replay input must fail");
        let rendered = format!("{error:#}");
        assert!(!rendered.contains(&basename));
        assert!(!rendered.contains(&path.display().to_string()));
    }

    #[tokio::test]
    async fn forensic_formatter_refusal_keeps_full_live_pcm_session() {
        use crate::presentation::emitter::PresentationEmitter;
        use codescribe_core::llm::provider::{CustomProvider, WireFamily};
        use codescribe_core::pipeline::contracts::EventSink;
        use sha2::{Digest, Sha256};
        use std::sync::{Arc, Mutex};
        let _ = tracing_subscriber::fmt().with_ansi(false).with_env_filter(
            "codescribe_core::stt::tail_provider=info,codescribe_core::pipeline::streaming::apple_live_session=info,codescribe_core::pipeline::streaming::session=info"
        ).try_init();

        struct LiveReplaySink {
            emitter: PresentationEmitter,
            events: Mutex<Vec<EngineEvent>>,
        }
        impl EventSink for LiveReplaySink {
            fn on_capture_opened(&self, session_id: &str, capture_epoch: u64) {
                self.emitter.on_capture_opened(session_id, capture_epoch);
            }
            fn on_event(&self, event: &EngineEvent) {
                self.events.lock().unwrap().push(event.clone());
                self.emitter.on_event(event);
            }
        }

        if std::env::var_os("CODESCRIBE_APPLE_STT_BRIDGE").is_none() {
            let installed =
                Path::new("/Applications/Codescribe.app/Contents/MacOS/codescribe-stt-bridge");
            if installed.is_file() {
                unsafe {
                    std::env::set_var("CODESCRIBE_APPLE_STT_BRIDGE", installed);
                    std::env::set_var("CODESCRIBE_BRIDGE_DISCLAIM", "1");
                }
            }
        }
        let wav = std::env::var("CODESCRIBE_STREAM_TEST_WAV").unwrap_or_else(|_| {
            Path::new(&std::env::var("HOME").expect("HOME"))
                .join(".vibecrafted/artifacts/vetcoders/codescribe/2026_1006/reports/word-ledger-integration/5c8ec179-0755-4e0f-a883-6e0cd2e7d93b.wav")
                .to_string_lossy()
                .into_owned()
        });
        // The incident recording is operator-private (never in the repo, never
        // on a CI runner). Without it this witness has nothing to replay.
        if !Path::new(&wav).is_file() {
            eprintln!("skipping forensic live PCM replay: fixture not present at {wav}");
            return;
        }
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&wav).unwrap())),
            "bbc3a7e22ad871e7f733a857d53c372143367b8d7ad58f7d98f607e63146df6b",
            "replay must use the exact 60.89 s incident recording"
        );
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || runtime.block_on(async move {
            let (samples, rate) = codescribe_core::audio::load_audio_file(Path::new(&wav)).unwrap();
            assert_eq!((samples.len(), rate), (2_922_496, 48_000));
            // Reserve a loopback address without serving a formatter. Every
            // attempt fails locally; no transcript or credentials leave the host.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
            drop(listener);
            let provider = CustomProvider::new("p0-replay-refusal", WireFamily::OpenAiResponses, &endpoint).unwrap();
            let settings = UserSettings {
                asr_mode: Some("local_power".into()),
                // This witness exercises ledger-backed live formatting, an
                // explicit opt-in independent of the product's direct default.
                seal_ledger_enabled: Some(true),
                apple_live_enabled: Some(true),
                formatting_level: Some("smart".into()),
                llm_formatting_provider: Some(format!("custom:{}", provider.id)),
                llm_formatting_model: Some("replay-refusal".into()),
                llm_custom_providers: vec![provider], ..UserSettings::default()
            };
            settings.save().expect("isolated replay settings");
            std::fs::copy(
                Path::new(&wav).parent().unwrap().join("p0-energy-calibration.json"),
                codescribe_core::config::energy_calibration_path(),
            ).expect("use the incident host's measured capture calibration");
            let snapshot = Arc::new(codescribe_core::config::Config::load_runtime_snapshot_without_keychain().unwrap());
            assert!(snapshot.seal_ledger_enabled());
            assert!(snapshot.apple_live_enabled());
            assert!(snapshot.seal_lane_armed());
            assert_eq!(snapshot.llm_lanes().formatting().endpoint(), endpoint);
            assert!(snapshot.llm_lanes().formatting().request_available());
            assert_eq!(snapshot.formatting_policy(), codescribe_core::config::FormattingPolicy::Smart);
            let calibration = snapshot.energy_calibration_for_capture("MacBook Pro Microphone", rate).unwrap();
            assert_eq!(calibration.version, "cal2-macbook-pro-microphone-1791099372411@48000hz");
            let (layer1, _) = codescribe_core::asr_session::layer1_decision(&snapshot);
            assert!(layer1.is_armed());
            let ledger = Arc::new(Mutex::new(AcousticLedger::new()));
            let buffer = Arc::new(tokio::sync::Mutex::new(String::new()));
            let mut sink = Arc::new(LiveReplaySink {
                emitter: PresentationEmitter::new_with_authority(buffer.clone(), None, None, None, Some(ledger.clone()), None),
                events: Mutex::new(Vec::new()),
            });
            let session_id = uuid::Uuid::new_v4().to_string();
            sink.on_capture_opened(&session_id, 1);
            let formatter_deadline = retained_formatter_deadline(&snapshot);
            let config = codescribe_core::pipeline::streaming::SessionConfig {
                session_id, capture_epoch: 1, runtime_settings: snapshot,
                live_formatting_agent: None, acoustic_ledger: ledger.clone(), sample_rate: rate,
                capture_device_name: Some("MacBook Pro Microphone".into()), language: Some("pl".into()),
                stream_log_path: None, utterance_silence_sec: settings.toggle_silence_sec,
                capture_turn: codescribe_core::audio::streaming_recorder::CaptureTurnIntent::HandsFree,
                layer1, lifecycle_events: None, terminal_audio: None, last_window_closed: None,
            };
            let started = std::time::Instant::now();
            codescribe_core::pipeline::streaming::replay_buffered_engine_session(&samples, config, sink.clone())
                .await.expect("the complete live PCM session must return");
            let raw_words_at_capture_end = buffer.lock().await.split_whitespace().count();
            assert!(raw_words_at_capture_end > 19, "Raw delivery must continue while optional formatter jobs are retained");
            // Match the live replay owner's bounded drain: formatter jobs may
            // still hold the sink after ASR has returned the full recording.
            tokio::time::timeout(formatter_deadline, async {
                loop {
                    if let Some(sink) = Arc::get_mut(&mut sink) {
                        sink.emitter.finish().await;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }).await.expect("retained formatter jobs must release the replay sink within their deadline");
            let delivered = buffer.lock().await.clone();
            let events = sink.events.lock().unwrap();
            let proposals = events.iter().filter(|event| matches!(event, EngineEvent::OccurrenceLabelProposal { .. })).count();
            let worker_failed = events.iter().any(|event| matches!(event, EngineEvent::NoSpeech { reason } if reason.starts_with("apple_live_stream_worker")));
            let observed = events.iter().filter_map(|event| match event {
                EngineEvent::SealCoverage { receipt, .. } => receipt.observed_samples,
                _ => None,
            }).max();
            let ledger = ledger.lock().unwrap();
            let last_covered = ledger.last_covered_sample().unwrap_or_default();
            let words = delivered.split_whitespace().count();
            eprintln!("p0_live_pcm_acceptance={}", serde_json::json!({
                "audio_samples": samples.len(), "sample_rate": rate,
                "elapsed_seconds": started.elapsed().as_secs_f64(),
                "events": events.len(), "formatter_proposals": proposals,
                "worker_failed": worker_failed, "observed_samples": observed,
                "last_covered_sample": last_covered, "delivered_words": words,
                "raw_words_at_capture_end": raw_words_at_capture_end,
                "warning_codes": events.iter().filter_map(|event| match event {
                    EngineEvent::Warning { code, .. } => Some(code), _ => None,
                }).collect::<std::collections::BTreeSet<_>>(),
            }));
            assert!(proposals > 0, "an armed formatter must actually return proposals");
            assert!(!worker_failed, "optional formatter failure must not terminate ASR");
            assert_eq!(observed, Some(samples.len() as u64), "the pipeline must observe the whole take");
            assert!(last_covered > 987_136, "ledger admission must continue beyond the P0 cutoff");
            assert!(words > 19, "delivery must include speech after the old 19-word prefix");
        })).await.expect("live PCM replay must join");
    }

    #[tokio::test]
    #[ignore = "explicit private PCM replay; needs signed Apple bridge and cached local Whisper"]
    async fn forensic_word_ledger_number_and_negation_acceptance() {
        use sha2::{Digest, Sha256};
        let wav = std::env::var("CODESCRIBE_STREAM_TEST_WAV")
            .expect("explicitly select the private audio fixture");
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&wav).unwrap())),
            "a11546e2e049bd99de2c6305d7f80c40d4328c6599ed0425208eb56c2bef2a95",
            "the oracle belongs to the supplied number/negation fixture"
        );
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || runtime.block_on(async move {
            let (samples, rate) = codescribe_core::audio::load_audio_file(Path::new(&wav)).unwrap();
            let settings = UserSettings { asr_mode: Some("local_power".into()), ..UserSettings::default() };
            settings.save().expect("isolated profile snapshot");
            // Bind the archived recording's actual measured capture profile.
            // The generic file replay intentionally has no microphone identity
            // and cannot qualify PCM; never substitute an invented energy floor.
            let capture_root = Path::new(&wav).ancestors().nth(3).unwrap();
            let calibration_path = capture_root.join("Library/Application Support/Codescribe/energy-calibration.json");
            std::fs::copy(calibration_path, codescribe_core::config::energy_calibration_path())
                .expect("copy measured calibration into the isolated test profile");
            let snapshot = std::sync::Arc::new(codescribe_core::config::Config::load_runtime_snapshot_without_keychain().unwrap());
            let calibration = snapshot.energy_calibration_for_capture("EarPods Microphone", rate).unwrap();
            assert_eq!(calibration.version, "cal2-earpods-microphone-1790960111192@48000hz");
            let (layer1, _) = codescribe_core::asr_session::layer1_decision(&snapshot);
            assert!(layer1.is_armed());
            let acoustic_ledger = std::sync::Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
            let config = codescribe_core::pipeline::streaming::SessionConfig {
                session_id: uuid::Uuid::new_v4().to_string(), capture_epoch: 1,
                runtime_settings: snapshot, live_formatting_agent: None,
                acoustic_ledger: acoustic_ledger.clone(), sample_rate: rate,
                capture_device_name: Some("EarPods Microphone".into()),
                language: Some("pl".into()), stream_log_path: None,
                utterance_silence_sec: settings.toggle_silence_sec,
                capture_turn: codescribe_core::audio::streaming_recorder::CaptureTurnIntent::HandsFree,
                layer1, lifecycle_events: None, terminal_audio: None, last_window_closed: None,
            };
            let started = std::time::Instant::now();
            let events = codescribe_core::pipeline::streaming::collect_buffered_engine_events_with_config(&samples, config)
                .await.expect("production replay must complete");
            let session = codescribe_core::audio::streaming_recorder::ProductionSessionReplay {
                tail_patch_receipt: codescribe_core::pipeline::streaming::TailPatchSessionReceipt::from_events(&events),
                events, acoustic_ledger, layer1_armed: true,
                streaming_engine_label: "live_apple".into(),
            };
            let ledger = session.acoustic_ledger.lock().unwrap();
            let mut reducer = TranscriptReducer::default();
            let mut seen_number = false;
            let mut regressions = 0;
            let mut revisions = 0;
            for event in &session.events {
                let revision = match event {
                    EngineEvent::LedgerMutation { observation, receipt, .. } =>
                        reducer.apply_ledger_mutation(&ledger, observation, receipt),
                    EngineEvent::LedgerSeal { receipt } => reducer.apply_ledger_seal(receipt),
                    EngineEvent::OccurrenceLabelProposal { proposal } => {
                        reducer.apply_occurrence_label_proposal(&ledger, proposal);
                        None
                    },
                    _ => None,
                };
                if let Some(revision) = revision {
                    revisions += 1;
                    let tokens = revision.rendered_text.split(|c: char| !c.is_alphanumeric()).collect::<Vec<_>>();
                    if seen_number && !tokens.contains(&"1286") { regressions += 1; }
                    seen_number |= tokens.contains(&"1286");
                }
            }
            let final_text = reducer.visible_projection();
            let normalized = final_text.split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty()).map(str::to_lowercase).collect::<Vec<_>>().join(" ");
            let number = normalized.split_whitespace().filter(|t| *t == "1286").count();
            let negation_one = normalized.contains("nie oddajemy agentowi");
            let negation_two = normalized.contains("nie stoimy w miejscu");
            eprintln!("word_ledger_pcm_acceptance={}", serde_json::json!({
                "audio_seconds": samples.len() as f64 / f64::from(rate),
                "elapsed_seconds": started.elapsed().as_secs_f64(),
                "event_count": session.events.len(),
                "warning_codes": session.events.iter().filter_map(|event| match event {
                    EngineEvent::Warning { code, .. } => Some(code), _ => None,
                }).collect::<std::collections::BTreeSet<_>>(),
                "revisions": revisions, "number_occurrences": number,
                "number_regressions": regressions, "negation_one": negation_one,
                "negation_two": negation_two, "words": normalized.split_whitespace().count(),
            }));
            assert!(revisions > 0);
            assert_eq!(number, 1, "one physical occurrence of the expected number");
            assert_eq!(regressions, 0, "a later revision must not erase the correct number");
            assert!(negation_one && negation_two, "both supplied negation contrasts must survive");
            let delivery = finish_replay_delivery(final_text.clone(), None, &session.streaming_engine_label).unwrap();
            assert_eq!(delivery.delivered_text, final_text);
        })).await.expect("file replay must join");
    }

    // Generic PCM replay has no capture identity/calibration. Keep this
    // historical private probe explicit; calibrated acceptance uses the
    // recorded-session replay card rather than a default profile.
    #[tokio::test]
    #[ignore = "private PCM probe needs authenticated capture setup; use codescribe replay SESSION for calibrated acceptance"]
    async fn forensic_saved_pcm_produces_authenticated_projection() {
        if std::env::var_os("CODESCRIBE_APPLE_STT_BRIDGE").is_none() {
            let installed =
                Path::new("/Applications/Codescribe.app/Contents/MacOS/codescribe-stt-bridge");
            if installed.is_file() {
                unsafe {
                    std::env::set_var("CODESCRIBE_APPLE_STT_BRIDGE", installed);
                    std::env::set_var("CODESCRIBE_BRIDGE_DISCLAIM", "1");
                }
            }
        }
        let operator_home = std::env::var("HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .filter(|h| h.join(".codescribe/data_assets").is_dir())
            .or_else(|| {
                std::env::var("CARGO_HOME").ok().and_then(|c| {
                    std::path::PathBuf::from(c)
                        .parent()
                        .map(std::path::Path::to_path_buf)
                })
            })
            .expect("operator home");
        let _models_dir = codescribe_core::test_isolation::EnvGuard::set(
            "CODESCRIBE_MODELS_DIR",
            operator_home.join(".codescribe/models"),
        );
        let wav = std::env::var("CODESCRIBE_STREAM_TEST_WAV").unwrap_or_else(|_| {
            operator_home
                .join(".codescribe/data_assets/01_no-to-dobra.wav")
                .to_string_lossy()
                .into_owned()
        });
        // Same contract as tests/assets/data_assets/README.md: a private clip
        // that is absent on this host (every CI runner) skips, it does not fail.
        if !Path::new(&wav).is_file() {
            eprintln!("skipping forensic saved PCM projection: fixture not present at {wav}");
            return;
        }
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || runtime.block_on(async move {
            let (samples, rate) = codescribe_core::audio::load_audio_file(Path::new(&wav))
                .expect("selected fixture must decode");
            let settings = UserSettings {
                asr_mode: Some("local_power".into()),
                ..UserSettings::default()
            };
            settings.save().expect("persist the isolated profile snapshot");
            let session = replay_production_session(&samples, rate, Some("pl".into()), &settings)
                .await.expect("production PCM session must return its evidence");
            let warning_codes = session.events.iter().filter_map(|event| match event {
                EngineEvent::Warning { code, .. } => Some(code.clone()),
                _ => None,
            }).collect::<Vec<_>>();
            let previews = session.events.iter()
                .filter(|event| matches!(event, EngineEvent::Preview { .. })).count();
            let granted_mutations = session.events.iter().filter(|event| match event {
                EngineEvent::LedgerMutation { receipt, .. } => receipt.grants_mutation(),
                _ => false,
            }).count();
            let seal_events = session.events.iter()
                .filter(|event| matches!(event, EngineEvent::LedgerSeal { .. })).count();
            let mut ledger = session.acoustic_ledger.lock().unwrap();
            let owners = ledger.occurrences().cloned().collect::<Vec<_>>();
            let pending_recovery = owners.iter().filter(|owner| ledger.text_recovery_pending(owner)).count();
            let committed_chars = owners.iter().map(|owner| ledger.text_of(owner).unwrap_or("").chars().count()).sum::<usize>();
            let projection = project_ledger_truth(&session.events, &mut ledger);
            eprintln!("forensic_pcm_evidence={}", serde_json::json!({
                "samples": samples.len(), "rate": rate, "events": session.events.len(),
                "previews": previews, "warning_codes": warning_codes,
                "granted_mutations": granted_mutations, "seal_events": seal_events,
                "owners": owners.len(), "committed_chars": committed_chars,
                "pending_recovery": pending_recovery, "layer1_armed": session.layer1_armed,
                "projection_nonempty": projection.as_ref().is_ok_and(|text| !text.trim().is_empty()),
            }));
            assert!(session.layer1_armed, "the selected isolated profile must arm Whisper");
            assert!(projection.is_ok_and(|text| !text.trim().is_empty()),
                "actual production PCM must create an authenticated transcript projection");
        })).await.expect("diagnostic worker must join");
    }
}
