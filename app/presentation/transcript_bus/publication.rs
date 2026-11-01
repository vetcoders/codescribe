//! Session lifecycle and reducer publication: start, revisions authenticated
//! by the ledger or by a direct capability, derived projections, the terminal
//! lifecycle line and the persistence writer that appends them.

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;

use crate::controller::{
    TranscriptProjectionAvailability, resolve_transcript_projection_availability,
};
use crate::presentation::emitter::{
    DerivedTranscriptProjection, ReducerAction, TranscriptRevision,
};

use super::envelope::*;
use super::routing::*;
use super::{
    ProjectedSealCoverageReceipt, TranscriptBus, TranscriptBusEvidenceEvent, TranscriptBusWriter,
};

impl TranscriptBus {
    /// Copy ownership from the recorder's physical capture-open callback.
    pub(crate) fn observe_capture_opened(&self, session_id: &str, capture_epoch: u64) {
        if self.session.session_id != session_id || capture_epoch == 0 {
            return;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !writer.ended && writer.capture_epoch.is_none() {
            writer.capture_epoch = Some(capture_epoch);
        }
    }

    /// The actual controller lifecycle must have completed this opened take.
    /// Start failures and superseded starts never authorize a human document.
    pub(crate) fn completed_capture_revision(
        &self,
        session_id: &str,
        capture_epoch: u64,
        source_revision: u64,
    ) -> bool {
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.session.session_id == session_id
            && writer.started
            && writer.ended
            && writer.capture_epoch == Some(capture_epoch)
            && matches!(
                writer.end_reason,
                Some(
                    TranscriptSessionEndReason::Completed
                        | TranscriptSessionEndReason::CoverageRefused
                        | TranscriptSessionEndReason::CoverageRefusedEmpty
                        | TranscriptSessionEndReason::DeliveryFailed
                        | TranscriptSessionEndReason::TranscriptionFailed
                )
            )
            && writer
                .last_projection
                .as_ref()
                .map_or(writer.capture_revision, |last| last.reducer_revision)
                == source_revision
    }

    pub(super) fn projection_availability(
        &self,
        has_text: bool,
        take_in_progress: bool,
        session_wav_exists: bool,
    ) -> TranscriptProjectionAvailability {
        let mut availability = resolve_transcript_projection_availability(
            has_text,
            take_in_progress,
            session_wav_exists,
            self.session.has_latched_target,
            self.session.latched_target_is_self,
        );
        if self.session.badge_only {
            availability.can_paste = false;
            availability.can_insert = false;
            availability.can_send_to_agent = false;
        }
        availability
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session.session_id
    }

    /// Publish a ledger-free reducer revision authenticated by its embedded
    /// direct capability. Rows keep the existing schemas; the acoustic receipt
    /// list stays honestly empty because no ledger attested this take. The
    /// quiet channel contract and the FIFO ordering match the ledger lane;
    /// the terminal mint records `direct_terminal` for the channel close.
    pub fn publish_direct_revision(
        &self,
        revision: &TranscriptRevision,
    ) -> Vec<TranscriptBusEvidenceEvent> {
        // Atomic refusal: no partial rows, sequence changes or last-render
        // update. The capability validator is owned by the reducer.
        let Some(capability) = revision.authenticates_direct_publication(&self.session.session_id)
        else {
            return Vec::new();
        };
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // Completion closes decoder admission, not explicit document operations.
        // Their reducer capability must continue the exact retained CAS/epoch.
        let operation = capability.document_revision;
        if let Some((source_revision, _)) = operation {
            let continues_terminal = capability.terminal
                && writer.direct_terminal.as_ref().is_some_and(|owner| {
                    owner.capture_epoch == capability.capture_epoch
                        && owner.revision == source_revision
                })
                && writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|last| last.reducer_revision == source_revision);
            if !continues_terminal {
                return Vec::new();
            }
        } else if writer.direct_terminal.is_some() {
            return Vec::new();
        }
        if writer
            .last_projection
            .as_ref()
            .is_some_and(|last| revision.revision <= last.reducer_revision)
        {
            return Vec::new();
        }
        if writer.ended && operation.is_none() {
            return Vec::new();
        }
        let phase = if operation.is_some() {
            TranscriptProjectionPhase::Formatted
        } else if capability.terminal {
            TranscriptProjectionPhase::Finalizing
        } else {
            TranscriptProjectionPhase::Listening
        };
        // Decoder completion does not end capture. Only the lifecycle end
        // releases document actions; later revisions retain its audio proof,
        // exactly like post-stop revisions in the ledger lane.
        let availability = if writer.ended {
            self.projection_availability(
                !revision.rendered_text.trim().is_empty(),
                false,
                writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|last| last.can_retranscribe),
            )
        } else {
            self.projection_availability(!revision.rendered_text.trim().is_empty(), true, false)
        };
        // Quiet delivery contract (Founder seal cc6c8248, 2026-09-29; only its
        // reopen half was replaced on 2026-10-26): identical for the direct
        // lane — one start row per document, the terminal row, intermediate
        // revisions in-process only.
        let channel_quiet = self.session.audience.is_some();
        let mut emitted = Vec::new();
        let mut persisted = Vec::new();
        for (document_index, entry) in revision.entries.iter().enumerate() {
            let event = TranscriptBusEvidenceEvent {
                schema: "codescribe.transcript-evidence.v1".to_string(),
                sequence: writer.sequence.saturating_add(1),
                emitted_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
                session_id: self.session.session_id.clone(),
                mode: self.session.mode,
                reducer_revision: revision.revision,
                reducer_action: if operation.is_some() {
                    "apply_direct_document_revision".to_string()
                } else if capability.terminal {
                    "apply_direct_transcript_terminal".to_string()
                } else {
                    "apply_direct_transcript".to_string()
                },
                occurrence_session_id: entry.occurrence.session.clone(),
                capture_epoch: entry.occurrence.capture_epoch,
                sample_start: entry.occurrence.sample_start,
                sample_end: entry.occurrence.sample_end,
                document_index: document_index as u64,
                label: entry.label.clone(),
                rendered_text: revision.rendered_text.clone(),
                delivery_text: None,
                phase,
                can_paste: availability.can_paste,
                can_insert: availability.can_insert,
                can_copy: availability.can_copy,
                can_retranscribe: availability.can_retranscribe,
                can_format: availability.can_format,
                can_send_to_agent: availability.can_send_to_agent,
                // The channel close itself marks the closing snapshot terminal,
                // exactly as in the ledger lane.
                terminal: operation.is_some(),
                lifecycle_terminal: false,
                delivery: TranscriptDelivery::Unattempted,
                audience: self.session.audience.clone(),
                // No ledger, no acoustic receipts: the capability above is the
                // whole authority behind this row.
                acoustic_receipts: Vec::new(),
                document_revision_receipt: capability.document_revision_receipt(),
                seal_coverage: None,
                comparison: None,
                consultation_presentations: Vec::new(),
                uncertain_spans: Vec::new(),
            };
            let starts_document = channel_quiet
                && !writer
                    .announced_documents
                    .contains(&(document_index as u64))
                && !revision.rendered_text.trim().is_empty();
            let persist = !channel_quiet || capability.terminal || starts_document;
            if persist {
                if starts_document {
                    writer.announced_documents.insert(document_index as u64);
                }
                writer.sequence = event.sequence;
                persisted.push(emitted.len());
            }
            writer.last_projection = Some(event.clone());
            emitted.push(event);
        }
        self.persist_revision_locked(&mut writer, &emitted, &persisted);
        if capability.terminal {
            writer.direct_terminal = Some(capability.clone());
        }
        emitted
    }

    /// Resolve the production path and open the session bus. Failure disables
    /// only observability; it must never stop microphone capture or delivery.
    pub fn open(session: TranscriptSession) -> Option<Self> {
        Some(Self::open_with_path(session, transcript_bus_path()))
    }

    /// The production fallback, with an explicit path for local failure
    /// fixtures and for channel sessions routed to a dedicated bus (W5).
    pub(crate) fn open_with_path(session: TranscriptSession, path: PathBuf) -> Self {
        match Self::open_at(session.clone(), path.clone(), None) {
            Ok(bus) => bus,
            Err(error) => {
                let bus = Self::with_writer(session, path, None);
                bus.log_write_error(error);
                bus
            }
        }
    }

    /// Open one lifecycle/sequence with a primary path and additional buses.
    /// Every descriptor comes from the shared registry used by compaction.
    pub(crate) fn open_with_paths(
        session: TranscriptSession,
        path: PathBuf,
        additional_paths: Vec<PathBuf>,
    ) -> Self {
        let bus = Self::open_with_path(session, path.clone());
        if additional_paths.is_empty() {
            return bus;
        }
        let mut writer = bus.writer.lock().unwrap_or_else(|error| error.into_inner());
        let mut destinations = vec![BusDestination {
            path,
            file: writer.file.take(),
        }];
        for path in additional_paths {
            if destinations.iter().any(|known| known.path == path) {
                continue;
            }
            let file = match Self::open_persistence_file(&path) {
                Ok(file) => Some(file),
                Err(error) => {
                    tracing::warn!(%error, bus = %path.display(), session_id = %bus.session.session_id, persistence = "disabled_for_session", "transcript bus destination could not open; other destinations continue");
                    None
                }
            };
            destinations.push(BusDestination { path, file });
        }
        writer.file = Some(Box::new(BusFanout {
            session_id: bus.session.session_id.clone(),
            destinations,
        }));
        drop(writer);
        bus
    }

    pub(super) fn with_writer(
        session: TranscriptSession,
        path: PathBuf,
        file: Option<Box<dyn Write + Send>>,
    ) -> Self {
        Self {
            session,
            path,
            writer: Mutex::new(TranscriptBusWriter {
                channel_recipients: None,
                co_audience: CoAudienceSource::Absent,
                file,
                sequence: 0,
                started: false,
                sealed: false,
                ended: false,
                end_reason: None,
                capture_epoch: None,
                capture_revision: 0,
                capture_coverage: None,
                capture_comparison: None,
                last_projection: None,
                channel_seal_published: false,
                channel_publication_evidence: None,
                direct_terminal: None,
                announced_documents: HashSet::new(),
            }),
            persistence_diagnostic_sink: Arc::new(|_| {}),
        }
    }

    /// Test seam: observe persistence failures directly, without a tracing
    /// subscriber. Production logging in `log_write_error` is unchanged.
    #[cfg(test)]
    pub(super) fn set_persistence_diagnostic_sink(&mut self, sink: PersistenceDiagnosticSink) {
        self.persistence_diagnostic_sink = sink;
    }

    /// Open an explicit path. Kept public for deterministic pipeline tests and
    /// embedders that already own an XDG/project state root.
    pub fn open_at(
        session: TranscriptSession,
        path: PathBuf,
        _sample_rate_override: Option<u32>,
    ) -> io::Result<Self> {
        let file = Self::open_persistence_file(&path)?;
        Ok(Self::with_writer(session, path, Some(file)))
    }

    pub(super) fn open_persistence_file(path: &Path) -> io::Result<Box<dyn Write + Send>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let shared = shared_bus_file(path)?;
        let mut file = shared.lock().unwrap_or_else(|error| error.into_inner());

        // A prior partial write is not an append boundary. Do not join a new
        // session onto it, truncate evidence, or retry the unknown payload.
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transcript bus has an incomplete trailing row; persistence disabled",
                ));
            }
        }
        drop(file);
        Ok(Box::new(SharedBusWriter(shared, path.to_path_buf())))
    }

    /// Announce the recording start exactly once, even if persistence fails.
    /// The controller owns when this happens; retries cannot append another start.
    pub fn publish_started(&self) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match self.ensure_started_locked(&mut writer) {
            Ok(true) => {
                tracing::info!(path = %self.path.display(), session_id = %self.session.session_id, mode = ?self.session.mode, "clean transcript bus session started");
            }
            Ok(false) => {}
            Err(error) => self.log_write_error(error),
        }
    }

    /// Journal a reducer-minted derived version without advancing the Raw
    /// projection, seal, lifecycle, or delivery disposition. Readers select
    /// this schema explicitly; acoustic document readers continue to see Raw.
    pub(crate) fn record_derived_projection(&self, projection: &DerivedTranscriptProjection) {
        if projection.session_id != self.session.session_id
            || !projection.authenticates_publication()
        {
            return;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = Self::append_projection_locked(&mut writer, projection) {
            self.log_write_error(error);
        }
    }

    /// UI-only projection of a reducer-authenticated version. It consumes a
    /// sequence but never changes the Bus's retained Raw document or emits a
    /// clean transcript event to an agent observer.
    pub(crate) fn derived_presentation_event(
        &self,
        projection: &DerivedTranscriptProjection,
    ) -> Option<TranscriptBusEvidenceEvent> {
        if !projection.authenticates_publication()
            || projection.session_id != self.session.session_id
        {
            return None;
        }
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut event = writer.last_projection.clone()?;
        if event.reducer_revision != projection.source_raw_revision
            || event.rendered_text != projection.source_raw_text
        {
            return None;
        }
        writer.sequence = writer.sequence.saturating_add(1);
        event.sequence = writer.sequence;
        event.emitted_at.clone_from(&projection.emitted_at);
        event.reducer_action = "derived_projection".into();
        event.label.clone_from(&projection.receipt_id);
        event.delivery_text = Some(projection.rendered_text.clone());
        event.lifecycle_terminal = false;
        event.delivery = TranscriptDelivery::Unattempted;
        event.uncertain_spans.clear();
        Some(event)
    }

    pub(crate) fn light_plus_deadline_event(
        &self,
        elapsed_ms: u128,
    ) -> Option<TranscriptBusEvidenceEvent> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut event = writer.last_projection.clone()?;
        let receipt = serde_json::json!({
            "schema": "codescribe.light-plus-tick.v1",
            "session_id": self.session.session_id,
            "source_raw_revision": event.reducer_revision,
            "status": "deadline_exceeded",
            "budget_ms": 2000,
            "elapsed_ms": elapsed_ms,
            "delivered": "unchanged_raw",
        });
        if let Err(error) = Self::append_projection_locked(&mut writer, &receipt) {
            self.log_write_error(error);
        }
        writer.sequence = writer.sequence.saturating_add(1);
        event.sequence = writer.sequence;
        event.reducer_action = "light_plus_tick_deadline".into();
        event.label = "Light+ skipped — delivered Raw".into();
        event.lifecycle_terminal = false;
        event.delivery = TranscriptDelivery::Unattempted;
        Some(event)
    }

    pub(crate) fn record_projection_delivery(
        &self,
        projection: &DerivedTranscriptProjection,
        payload: &str,
        disposition: TranscriptDelivery,
    ) {
        if projection.session_id != self.session.session_id
            || !projection.authenticates_publication()
        {
            return;
        }
        let receipt = serde_json::json!({
            "schema": "codescribe.projection-delivery.v1",
            "session_id": projection.session_id,
            "source_raw_revision": projection.source_raw_revision,
            "source_state": projection.source_state,
            "source_raw_text": projection.source_raw_text,
            "projection_receipt": projection.receipt_id,
            "requested_mode": projection.requested_mode,
            "selected_mode": projection.delivered_mode,
            "payload_utf8": payload,
            "disposition": disposition,
        });
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = Self::append_projection_locked(&mut writer, &receipt) {
            self.log_write_error(error);
        }
    }

    /// Publish the session's terminal lifecycle line exactly once, and only
    /// after a start was announced in process. Text-free: it carries no
    /// transcript authority (evidence seals, if any, precede it) — it tells an
    /// in-process observer the controller left this session, even when zero
    /// occurrences sealed. A file tailer may miss this line on persistence loss. `reason` is the typed cause; a session that was
    /// superseded or failed before recording says so instead of masquerading
    /// as a completed take.
    ///
    /// `delivery` is the controller's own disposition for the take. The Bus
    /// copies it; it never infers a destination from phase, label or text.
    /// An agent channel cannot end before its joined tail publishes the final
    /// snapshot: this lifecycle row would flush the follower's remembered text.
    pub fn publish_ended(
        &self,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
    ) -> Option<TranscriptBusEvidenceEvent> {
        self.publish_ended_with_delivery_text(reason, session_wav_exists, delivery, None)
    }

    /// End a session while keeping a delivery-only payload distinct from the
    /// reducer-owned document. Only the controller's composer route uses it.
    pub fn publish_ended_with_delivery_text(
        &self,
        reason: TranscriptSessionEndReason,
        session_wav_exists: bool,
        delivery: TranscriptDelivery,
        delivery_text: Option<String>,
    ) -> Option<TranscriptBusEvidenceEvent> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !writer.started || writer.ended {
            return None;
        }
        if self.session.mode == TranscriptMode::Agent
            && self.session.audience.is_some()
            && !writer.channel_seal_published
        {
            return None;
        }
        let has_text = writer
            .last_projection
            .as_ref()
            .is_some_and(|projection| !projection.rendered_text.trim().is_empty());
        let phase = match reason {
            TranscriptSessionEndReason::Completed if has_text => {
                TranscriptProjectionPhase::Formatted
            }
            TranscriptSessionEndReason::Completed | TranscriptSessionEndReason::ChordPrefix => {
                TranscriptProjectionPhase::NoSpeech
            }
            TranscriptSessionEndReason::CoverageRefused if has_text => {
                TranscriptProjectionPhase::CoverageRefused
            }
            TranscriptSessionEndReason::CoverageRefused
            | TranscriptSessionEndReason::CoverageRefusedEmpty
            | TranscriptSessionEndReason::DeliveryFailed
            | TranscriptSessionEndReason::StartSuperseded
            | TranscriptSessionEndReason::StartFailed
            | TranscriptSessionEndReason::TranscriptionFailed => TranscriptProjectionPhase::Error,
        };
        let availability = self.projection_availability(has_text, false, session_wav_exists);
        let event = CleanTranscriptEvent {
            schema: "codescribe.transcript.v1".to_string(),
            sequence: 0,
            session_id: String::new(),
            mode: self.session.mode,
            utterance_id: None,
            emitted_at: String::new(),
            status: "session_ended".to_string(),
            sample_rate_hz: None,
            capture_epoch: None,
            sample_start: None,
            sample_end: None,
            audio_start_seconds: None,
            audio_end_seconds: None,
            text: String::new(),
            phase,
            can_paste: availability.can_paste,
            can_insert: availability.can_insert,
            can_copy: availability.can_copy,
            can_retranscribe: availability.can_retranscribe,
            can_format: availability.can_format,
            can_send_to_agent: availability.can_send_to_agent,
            terminal: true,
            segments: Vec::new(),
            words: Vec::new(),
            coverage: None,
            pipeline_session_id: None,
            end_reason: Some(reason),
            // Ledger-observed: authorship is the app, expressed by absence.
            source: None,
        };
        if let Err(error) = self.write_event_locked(&mut writer, event) {
            self.log_write_error(error);
        }
        writer.ended = true;
        writer.end_reason = Some(reason);
        tracing::info!(path = %self.path.display(), session_id = %self.session.session_id, sealed = writer.sealed, ?reason, "clean transcript bus session ended");
        let mut terminal =
            writer
                .last_projection
                .clone()
                .unwrap_or_else(|| TranscriptBusEvidenceEvent {
                    schema: "codescribe.transcript-evidence.v1".to_string(),
                    sequence: writer.sequence,
                    emitted_at: String::new(),
                    session_id: self.session.session_id.clone(),
                    mode: self.session.mode,
                    reducer_revision: writer.capture_revision,
                    reducer_action: "session_ended".to_string(),
                    occurrence_session_id: String::new(),
                    capture_epoch: writer.capture_epoch.unwrap_or(0),
                    sample_start: 0,
                    sample_end: 0,
                    document_index: 0,
                    label: String::new(),
                    rendered_text: String::new(),
                    delivery_text: None,
                    phase,
                    can_paste: availability.can_paste,
                    can_insert: availability.can_insert,
                    can_copy: availability.can_copy,
                    can_retranscribe: availability.can_retranscribe,
                    can_format: availability.can_format,
                    can_send_to_agent: availability.can_send_to_agent,
                    terminal: true,
                    lifecycle_terminal: true,
                    delivery,
                    audience: self.session.audience.clone(),
                    acoustic_receipts: Vec::new(),
                    document_revision_receipt: None,
                    consultation_presentations: Vec::new(),
                    seal_coverage: writer.capture_coverage.clone(),
                    comparison: writer.capture_comparison.clone(),
                    uncertain_spans: Vec::new(),
                });
        terminal.sequence = writer.sequence;
        terminal.emitted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        terminal.reducer_action = "session_ended".to_string();
        terminal.phase = phase;
        terminal.can_paste = availability.can_paste;
        terminal.can_insert = availability.can_insert;
        terminal.can_copy = availability.can_copy;
        terminal.can_retranscribe = availability.can_retranscribe;
        terminal.can_format = availability.can_format;
        terminal.can_send_to_agent = availability.can_send_to_agent;
        terminal.terminal = true;
        // This projection is cloned from the last committed evidence
        // event, which was not a lifecycle line. Say what it now is.
        terminal.lifecycle_terminal = true;
        // The last committed evidence event carried `Unattempted`; the
        // lifecycle line is the one place a disposition is stated.
        terminal.delivery = delivery;
        terminal.delivery_text = delivery_text;
        writer.last_projection = Some(terminal.clone());
        Some(terminal)
    }

    /// The resolved path consumed by an external NDJSON tailer.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn ensure_started_locked(
        &self,
        writer: &mut TranscriptBusWriter,
    ) -> io::Result<bool> {
        if writer.started {
            return Ok(false);
        }
        writer.started = true;
        self.write_event_locked(
            writer,
            CleanTranscriptEvent {
                schema: "codescribe.transcript.v1".to_string(),
                sequence: 0,
                session_id: String::new(),
                mode: self.session.mode,
                utterance_id: None,
                emitted_at: String::new(),
                status: "session_started".to_string(),
                sample_rate_hz: None,
                capture_epoch: None,
                sample_start: None,
                sample_end: None,
                audio_start_seconds: None,
                audio_end_seconds: None,
                text: String::new(),
                phase: TranscriptProjectionPhase::Listening,
                can_paste: false,
                can_insert: false,
                can_copy: false,
                can_retranscribe: false,
                can_format: false,
                can_send_to_agent: false,
                terminal: false,
                segments: Vec::new(),
                words: Vec::new(),
                coverage: None,
                pipeline_session_id: None,
                end_reason: None,
                // Ledger-observed: authorship is the app, expressed by absence.
                source: None,
            },
        )?;
        Ok(true)
    }

    pub(super) fn write_event_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        mut event: CleanTranscriptEvent,
    ) -> io::Result<()> {
        let next_sequence = writer.sequence.saturating_add(1);
        event.sequence = next_sequence;
        event.session_id.clone_from(&self.session.session_id);
        event.mode = self.session.mode;
        event.emitted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);

        writer.sequence = next_sequence;
        if event.status == "session_ended" {
            // Preserve the reducer's actual source revision even without a
            // prior lexical entry. This metadata does not create a document.
            let mut row = serde_json::to_value(&event).map_err(io::Error::other)?;
            row["reducer_revision"] = serde_json::json!(
                writer
                    .last_projection
                    .as_ref()
                    .map_or(writer.capture_revision, |last| last.reducer_revision)
            );
            row["capture_epoch"] = serde_json::json!(writer.capture_epoch);
            if writer.last_projection.is_none() {
                row["seal_coverage"] =
                    serde_json::to_value(&writer.capture_coverage).map_err(io::Error::other)?;
                row["comparison"] =
                    serde_json::to_value(&writer.capture_comparison).map_err(io::Error::other)?;
            }
            Self::append_projection_locked(writer, &row)
        } else {
            Self::append_projection_locked(writer, &event)
        }
    }

    pub(super) fn write_evidence_event_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        event: &TranscriptBusEvidenceEvent,
    ) -> io::Result<()> {
        writer.sequence = event.sequence;
        Self::append_projection_locked(writer, event)
    }

    pub(super) fn append_projection_locked(
        writer: &mut TranscriptBusWriter,
        event: &impl Serialize,
    ) -> io::Result<()> {
        if writer.file.is_none() {
            return Ok(());
        }
        // Read the cohort before borrowing the file: one snapshot per row.
        let co_audience = writer
            .channel_recipients
            .as_ref()
            .and_then(|_| writer.co_audience.members());
        let Some(file) = writer.file.as_mut() else {
            return Ok(());
        };
        let result = (|| {
            let encoded = if let Some(recipients) = &writer.channel_recipients {
                let mut value = serde_json::to_value(event).map_err(io::Error::other)?;
                if let Some(fields) = value.as_object_mut() {
                    fields.insert("recipients".into(), recipients["recipients"].clone());
                    fields.insert("channel".into(), recipients["channel"].clone());
                    if let Some((basis, members)) = &co_audience {
                        fields.insert(
                            "co_audience".into(),
                            serde_json::to_value(members).map_err(io::Error::other)?,
                        );
                        fields.insert("co_audience_basis".into(), (*basis).into());
                    }
                }
                crate::presentation::transcript_bus_maintenance::generation::encode(&value)?
            } else {
                crate::presentation::transcript_bus_maintenance::generation::encode(event)?
            };
            file.write_all(&encoded)?;
            file.flush()
        })();
        if result.is_err() {
            // write_all may already have appended a prefix; flush failure may
            // leave a complete row. Neither permits retry or sequence reuse.
            writer.file = None;
        }
        result
    }

    pub(super) fn log_write_error(&self, error: io::Error) {
        let file = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("transcript-events.jsonl");
        let diagnostic = PersistenceDiagnostic {
            session_id: &self.session.session_id,
            file,
            error: &error,
        };
        (self.persistence_diagnostic_sink)(&diagnostic);
        tracing::warn!(error = %diagnostic.error, file = diagnostic.file, session_id = %diagnostic.session_id, persistence = "disabled_for_session", "clean transcript persistence unavailable; live projection continues without append proof");
    }

    /// Retain the authenticated source CAS and diagnostics of a refused empty
    /// capture for the lifecycle end; it has no lexical document to publish.
    pub(super) fn retain_capture_revision(&self, revision: &TranscriptRevision) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !writer.ended && revision.revision > writer.capture_revision {
            writer.capture_revision = revision.revision;
            writer.capture_coverage = revision
                .seal_coverage
                .as_ref()
                .map(ProjectedSealCoverageReceipt::from);
            writer.capture_comparison = revision
                .comparison
                .as_ref()
                .map(ProjectedTranscriptComparisonReceipt::from);
        }
    }

    /// Canvas phase of an authenticated revision. A document rewrite cannot
    /// clear the ledger's coverage verdict or an earlier error.
    pub(super) fn revision_phase(
        writer: &TranscriptBusWriter,
        revision: &TranscriptRevision,
        reducer_action: &str,
        is_user_revision: bool,
    ) -> TranscriptProjectionPhase {
        let phase = if is_user_revision {
            TranscriptProjectionPhase::Formatted
        } else if reducer_action == "record_ledger_terminal_seal" {
            TranscriptProjectionPhase::Finalizing
        } else {
            TranscriptProjectionPhase::Listening
        };
        // The authenticated reducer revision carries the ledger's coverage
        // verdict even before session_ended. A document rewrite cannot clear
        // that verdict; the ended row also preserves it for later revisions.
        let phase = if is_user_revision
            && (revision
                .seal_coverage
                .as_ref()
                .is_some_and(|coverage| !coverage.status.is_complete())
                || writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|event| event.phase == TranscriptProjectionPhase::CoverageRefused))
        {
            TranscriptProjectionPhase::CoverageRefused
        } else {
            phase
        };
        if is_user_revision {
            writer
                .last_projection
                .as_ref()
                .map(|last| last.phase)
                .filter(|phase| {
                    matches!(
                        phase,
                        TranscriptProjectionPhase::Error
                            | TranscriptProjectionPhase::CoverageRefused
                    )
                })
                .unwrap_or(phase)
        } else {
            phase
        }
    }

    /// Document actions offered by a revision: a user revision keeps the
    /// actions the take already had; a live revision is still recording.
    pub(super) fn revision_availability(
        &self,
        writer: &TranscriptBusWriter,
        revision: &TranscriptRevision,
        is_user_revision: bool,
    ) -> TranscriptProjectionAvailability {
        if is_user_revision && revision.entries.is_empty() {
            self.projection_availability(
                !revision.rendered_text.trim().is_empty(),
                false,
                writer
                    .last_projection
                    .as_ref()
                    .is_some_and(|last| last.can_retranscribe),
            )
        } else if is_user_revision {
            writer
                .last_projection
                .as_ref()
                .map(|projection| TranscriptProjectionAvailability {
                    can_paste: projection.can_paste,
                    can_insert: projection.can_insert,
                    can_copy: !revision.rendered_text.trim().is_empty(),
                    can_retranscribe: projection.can_retranscribe,
                    can_format: projection.can_format,
                    can_send_to_agent: projection.can_send_to_agent,
                })
                .unwrap_or_else(|| {
                    self.projection_availability(
                        !revision.rendered_text.trim().is_empty(),
                        false,
                        false,
                    )
                })
        } else {
            self.projection_availability(!revision.rendered_text.trim().is_empty(), true, false)
        }
    }

    /// A user revision of a take whose capture left no ledger entries: one
    /// document row, admitted only after the completed lifecycle of that capture.
    pub(super) fn publish_document_revision_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        revision: &TranscriptRevision,
        reducer_action: &str,
        phase: TranscriptProjectionPhase,
        availability: &TranscriptProjectionAvailability,
    ) -> Vec<TranscriptBusEvidenceEvent> {
        let ReducerAction::ApplyUserRevision { receipt } = &revision.action else {
            return Vec::new();
        };
        if !writer.started
            || !writer.ended
            || writer.capture_epoch != receipt.capture_epoch
            || !matches!(
                writer.end_reason,
                Some(
                    TranscriptSessionEndReason::Completed
                        | TranscriptSessionEndReason::CoverageRefused
                        | TranscriptSessionEndReason::CoverageRefusedEmpty
                        | TranscriptSessionEndReason::DeliveryFailed
                        | TranscriptSessionEndReason::TranscriptionFailed
                )
            )
        {
            return Vec::new();
        }
        let event = TranscriptBusEvidenceEvent {
            schema: "codescribe.transcript-evidence.v1".to_string(),
            sequence: writer.sequence.saturating_add(1),
            emitted_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
            session_id: self.session.session_id.clone(),
            mode: self.session.mode,
            reducer_revision: revision.revision,
            reducer_action: reducer_action.to_string(),
            occurrence_session_id: String::new(),
            capture_epoch: receipt.capture_epoch.unwrap_or(0),
            sample_start: 0,
            sample_end: 0,
            document_index: 0,
            label: String::new(),
            rendered_text: revision.rendered_text.clone(),
            delivery_text: None,
            phase,
            can_paste: availability.can_paste,
            can_insert: availability.can_insert,
            can_copy: availability.can_copy,
            can_retranscribe: availability.can_retranscribe,
            can_format: availability.can_format,
            can_send_to_agent: availability.can_send_to_agent,
            terminal: true,
            lifecycle_terminal: false,
            delivery: TranscriptDelivery::Unattempted,
            audience: self.session.audience.clone(),
            acoustic_receipts: Vec::new(),
            document_revision_receipt: Some(receipt.clone()),
            seal_coverage: revision
                .seal_coverage
                .as_ref()
                .map(ProjectedSealCoverageReceipt::from),
            comparison: revision
                .comparison
                .as_ref()
                .map(ProjectedTranscriptComparisonReceipt::from),
            consultation_presentations: Vec::new(),
            uncertain_spans: Vec::new(),
        };
        if let Err(error) = self.write_evidence_event_locked(writer, &event) {
            self.log_write_error(error);
        }
        writer.last_projection = Some(event.clone());
        vec![event]
    }

    /// Append one durable revision: the shared document once, then the
    /// differing fields of every other persisted occurrence.
    pub(super) fn persist_revision_locked(
        &self,
        writer: &mut TranscriptBusWriter,
        emitted: &[TranscriptBusEvidenceEvent],
        persisted: &[usize],
    ) {
        if let Some(&first) = persisted.first() {
            let durable = DurableRevision {
                document: &emitted[first],
                persistence_encoding: "shared-revision.v1",
                occurrence_rows: persisted
                    .iter()
                    .skip(1)
                    .map(|&index| DurableOccurrence::from(&emitted[index]))
                    .collect(),
            };
            if let Err(error) = Self::append_projection_locked(writer, &durable) {
                self.log_write_error(error);
            }
        }
    }
}
