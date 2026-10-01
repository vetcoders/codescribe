//! Diagnostic history only: no transcript authority, capture/UI IO or WAV decode.
//! Missing terminal records, overflow and unsupported replay inputs stay explicit.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::thread::JoinHandle;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, LayerDecisionReceipt, MutationReceipt,
    ObservationIdentity, OccurrenceIdentity, WordSlot,
};

const SCHEMA: &str = "codescribe.decision-trail.v1";
type CaptureKey = (String, u64);
static SINKS: OnceLock<Mutex<BTreeMap<CaptureKey, Weak<SenderState>>>> = OnceLock::new();

struct SenderState {
    sender: Mutex<Option<mpsc::SyncSender<TrailRecord>>>,
    dropped: Arc<AtomicU64>,
    origin: Instant,
}

/// Exact input to the production admission corridor, after word-pin reconciliation.
/// This does not pretend to be the original, unreconciled engine hypothesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailAdmission {
    pub source_slots: Vec<WordSlot>,
    pub offered_slots: Option<Vec<WordSlot>>,
    pub slot_revision: bool,
    pub capture_rate_hz: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailDecision {
    pub ordinal: usize,
    pub receipt_id: String,
    pub producer: String,
    pub layer: String,
    pub observation: ObservationIdentity,
    pub candidate_label: String,
    pub candidate_tokens: Vec<String>,
    pub verdict: MutationReceipt,
    pub predecessor_ordinal: Option<usize>,
    pub input: Option<TrailAdmission>,
    pub result_slots: Vec<WordSlot>,
    // Reducer revisions and job handoff times are separate authorities. Null
    // means uninstrumented, never zero or an inferred successful handoff.
    pub revision_before: Option<u64>,
    pub revision_after: Option<u64>,
    pub scheduled_ns: Option<u64>,
    pub returned_ns: Option<u64>,
    pub transferred_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrailEvent {
    Start {
        sample_clock: String,
    },
    Qualification {
        evidence: AcousticEvidence,
        calibration: EnergyCalibration,
    },
    Decision {
        decision: Box<TrailDecision>,
    },
    Frontier {
        occurrence: OccurrenceIdentity,
        operation: String,
        producers: Vec<super::acoustic_ledger::ObservationProducer>,
    },
    Projection {
        revision_before: u64,
        revision_after: u64,
        action: String,
        decision_receipts: Vec<String>,
    },
    End {
        dropped_records: u64,
        persistence_complete: bool,
        evidence_complete: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailRecord {
    pub schema: String,
    pub session: String,
    pub capture_epoch: u64,
    pub observed_ns: u64,
    #[serde(flatten)]
    pub event: TrailEvent,
}

/// Session-lifetime ownership. Drop drains and joins the IO worker on the STT
/// worker, never on capture or UI. Ordinary ledger tests have no registered sink.
pub(crate) struct TrailSink {
    key: CaptureKey,
    state: Arc<SenderState>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

pub fn trail_path(root: &Path, session: &str) -> io::Result<PathBuf> {
    if !(8..=80).contains(&session.len())
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(root.join("sessions").join(format!("{session}.trail.jsonl")))
}

impl TrailSink {
    pub(crate) fn open_in(
        root: &Path,
        session: &str,
        epoch: u64,
        capacity: usize,
    ) -> io::Result<Self> {
        if capacity == 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let path = trail_path(root, session)?;
        let key = (session.to_owned(), epoch);
        let mut sinks = SINKS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| io::Error::other("trail registry poisoned"))?;
        if sinks.get(&key).and_then(Weak::upgrade).is_some() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let state = Arc::new(SenderState {
            sender: Mutex::new(Some(sender)),
            dropped: dropped.clone(),
            origin: Instant::now(),
        });
        let session = session.to_owned();
        let worker = std::thread::Builder::new()
            .name("codescribe-trail-io".into())
            .spawn(move || {
                fs::create_dir_all(
                    path.parent()
                        .ok_or_else(|| io::Error::other("trail parent absent"))?,
                )?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(path)?;
                let mut write = |record: &TrailRecord| -> io::Result<()> {
                    serde_json::to_writer(&mut file, record).map_err(io::Error::other)?;
                    file.write_all(b"\n")
                };
                write(&TrailRecord {
                    schema: SCHEMA.into(),
                    session: session.clone(),
                    capture_epoch: epoch,
                    observed_ns: 0,
                    event: TrailEvent::Start {
                        sample_clock: "capture_samples".into(),
                    },
                })?;
                let mut last_ns = 0;
                for record in receiver {
                    last_ns = record.observed_ns;
                    write(&record)?;
                }
                let count = dropped.load(Ordering::Relaxed);
                write(&TrailRecord {
                    schema: SCHEMA.into(),
                    session,
                    capture_epoch: epoch,
                    observed_ns: last_ns,
                    event: TrailEvent::End {
                        dropped_records: count,
                        persistence_complete: count == 0,
                        evidence_complete: false,
                    },
                })?;
                file.flush()
            })?;
        sinks.insert(key.clone(), Arc::downgrade(&state));
        Ok(Self {
            key,
            state,
            worker: Some(worker),
        })
    }
}

impl Drop for TrailSink {
    fn drop(&mut self) {
        if let Some(sinks) = SINKS.get()
            && let Ok(mut sinks) = sinks.lock()
        {
            sinks.remove(&self.key);
        }
        if let Ok(mut sender) = self.state.sender.lock() {
            sender.take();
        }
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(kind = ?error.kind(), "decision trail incomplete: IO failed")
                }
                Err(_) => tracing::warn!("decision trail incomplete: IO worker panicked"),
            }
        }
    }
}

fn state(occurrence: &OccurrenceIdentity) -> Option<Arc<SenderState>> {
    SINKS
        .get()?
        .lock()
        .ok()?
        .get(&(occurrence.session.clone(), occurrence.capture_epoch))?
        .upgrade()
}

pub(super) fn is_enabled(occurrence: &OccurrenceIdentity) -> bool {
    state(occurrence).is_some()
}

/// The only enqueue corridor; JSON encoding and filesystem access live in the worker.
pub(super) fn record(occurrence: &OccurrenceIdentity, event: TrailEvent) {
    let Some(registry) = SINKS.get() else {
        return;
    };
    let Ok(sinks) = registry.lock() else {
        return;
    };
    let Some(state) = sinks
        .get(&(occurrence.session.clone(), occurrence.capture_epoch))
        .and_then(Weak::upgrade)
    else {
        return;
    };
    let record = TrailRecord {
        schema: SCHEMA.into(),
        session: occurrence.session.clone(),
        capture_epoch: occurrence.capture_epoch,
        observed_ns: u64::try_from(state.origin.elapsed().as_nanos()).unwrap_or(u64::MAX),
        event,
    };
    let sent = state.sender.lock().ok().is_some_and(|sender| {
        sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(record).is_ok())
    });
    if !sent {
        state.dropped.fetch_add(1, Ordering::Relaxed);
    }
    // Keep the registry guard through enqueue and overflow accounting. Drop
    // cannot unregister/close the queue until these records are accounted for.
    drop(sinks);
}

/// Passive reducer revision receipt; does not publish a document or return authority.
pub fn record_projection(
    occurrence: &OccurrenceIdentity,
    before: u64,
    after: u64,
    action: &str,
    receipts: Vec<String>,
) {
    record(
        occurrence,
        TrailEvent::Projection {
            revision_before: before,
            revision_after: after,
            action: action.into(),
            decision_receipts: receipts,
        },
    );
}

pub(super) fn decision(
    ledger: &AcousticLedger,
    entry: &LayerDecisionReceipt,
    input: Option<TrailAdmission>,
) {
    if !is_enabled(&entry.observation.occurrence) {
        return;
    }
    record(
        &entry.observation.occurrence,
        TrailEvent::Decision {
            decision: Box::new(TrailDecision {
                ordinal: entry.ordinal,
                receipt_id: entry.receipt_id.clone(),
                producer: entry.producer().as_str().into(),
                layer: entry.layer().into(),
                observation: entry.observation.clone(),
                candidate_label: entry.candidate_label.clone(),
                candidate_tokens: entry.candidate_tokens.clone(),
                verdict: entry.decision.clone(),
                predecessor_ordinal: entry.predecessor_ordinal,
                input,
                result_slots: ledger
                    .slots_of(&entry.observation.occurrence)
                    .unwrap_or(&[])
                    .to_vec(),
                revision_before: None,
                revision_after: None,
                scheduled_ns: None,
                returned_ns: None,
                transferred_ns: None,
            }),
        },
    );
}

pub fn read_trail(path: &Path) -> io::Result<Vec<TrailRecord>> {
    BufReader::new(std::fs::File::open(path)?)
        .lines()
        .map(|line| {
            let record: TrailRecord = serde_json::from_str(&line?).map_err(io::Error::other)?;
            if record.schema != SCHEMA {
                return Err(io::Error::other("unsupported decision trail schema"));
            }
            Ok(record)
        })
        .collect()
}

/// Per-pin fate, keyed by PCM geometry. Identical words remain separate rows.
/// Labels without word geometry are explicitly unpinned diagnostic hypotheses.
pub fn render_trace(records: &[TrailRecord], word: Option<&str>) -> String {
    let mut output = String::new();
    let complete = records.last().is_some_and(|row| {
        matches!(
            row.event,
            TrailEvent::End {
                persistence_complete: true,
                dropped_records: 0,
                ..
            }
        )
    });
    output.push_str(if complete {
        "Persistence complete; N2/revision evidence incomplete.\n"
    } else {
        "INCOMPLETE TRAIL: missing end, overflow or write failure.\n"
    });
    for row in records {
        match &row.event {
            TrailEvent::Frontier {
                occurrence,
                operation,
                producers,
            } => {
                output.push_str(&format!(
                    "frontier {}..{} {} {:?} at {} ns\n",
                    occurrence.sample_start,
                    occurrence.sample_end,
                    operation,
                    producers,
                    row.observed_ns
                ));
            }
            TrailEvent::Projection {
                revision_before,
                revision_after,
                action,
                decision_receipts,
            } => {
                output.push_str(&format!(
                    "reducer {} -> {} {} receipts={:?} at {} ns\n",
                    revision_before, revision_after, action, decision_receipts, row.observed_ns
                ));
            }
            _ => {}
        }
        let TrailEvent::Decision { decision } = &row.event else {
            continue;
        };
        let source = decision
            .input
            .as_ref()
            .map(|input| input.source_slots.as_slice())
            .unwrap_or(&[]);
        if word.is_some_and(|word| {
            !decision
                .candidate_tokens
                .iter()
                .any(|token| token.eq_ignore_ascii_case(word))
                && !source
                    .iter()
                    .chain(&decision.result_slots)
                    .any(|pin| pin.text.eq_ignore_ascii_case(word))
        }) {
            continue;
        }
        output.push_str(&format!("{} epoch={} occurrence={}..{} producer={} request={} generation={} verdict={} predecessor={:?}\n  engine literal: {:?}\n",
            decision.receipt_id, row.capture_epoch, decision.observation.occurrence.sample_start,
            decision.observation.occurrence.sample_end, decision.producer, decision.observation.request,
            decision.observation.generation, decision.verdict.as_str(), decision.predecessor_ordinal, decision.candidate_label));
        let mut pins = BTreeMap::new();
        for pin in source.iter().chain(&decision.result_slots) {
            pins.entry((pin.sample_start, pin.sample_end))
                .or_insert(pin);
        }
        if pins.is_empty() {
            output.push_str("  unpinned hypothesis: no word timestamps invented\n");
        }
        for ((start, end), pin) in pins {
            let before = source
                .iter()
                .find(|p| (p.sample_start, p.sample_end) == (start, end));
            let after = decision
                .result_slots
                .iter()
                .find(|p| (p.sample_start, p.sample_end) == (start, end));
            if word.is_some_and(|word| {
                !pin.text.eq_ignore_ascii_case(word)
                    && !after.is_some_and(|p| p.text.eq_ignore_ascii_case(word))
            }) {
                continue;
            }
            let fate = match (before, after) {
                (Some(a), Some(b)) if a.text == b.text => "retained",
                (Some(_), Some(_)) => "changed",
                (Some(_), None) => "lost_or_resegmented",
                _ => "introduced",
            };
            output.push_str(&format!(
                "  pin {start}..{end} {fate}: {:?} -> {:?}\n",
                before.map(|p| p.text.as_str()),
                after.map(|p| p.text.as_str())
            ));
        }
    }
    output
}

/// Replay recorded admissions through production code. The caller applies each
/// real mutation receipt to its production reducer; no WAV/model work is involved.
/// Unsupported corridors fail explicitly instead of fabricating historic inputs.
pub fn replay_decisions(
    records: &[TrailRecord],
    mut project: impl FnMut(&AcousticLedger, &ObservationIdentity, &MutationReceipt),
) -> io::Result<AcousticLedger> {
    let first = records
        .first()
        .ok_or_else(|| io::Error::other("empty trail"))?;
    if !matches!(first.event, TrailEvent::Start { .. }) {
        return Err(io::Error::other("trail lacks start"));
    }
    let identity = OccurrenceIdentity::new(first.session.clone(), first.capture_epoch, 0, 1);
    if is_enabled(&identity) {
        return Err(io::Error::other(
            "cannot replay into an active trail session",
        ));
    }
    let mut ledger = AcousticLedger::new();
    for row in records {
        if row.schema != SCHEMA
            || row.session != first.session
            || row.capture_epoch != first.capture_epoch
        {
            return Err(io::Error::other("trail identity or schema differs"));
        }
        let occurrence = match &row.event {
            TrailEvent::Qualification { evidence, .. } => Some(&evidence.occurrence),
            TrailEvent::Decision { decision } => Some(&decision.observation.occurrence),
            TrailEvent::Frontier { occurrence, .. } => Some(occurrence),
            _ => None,
        };
        if occurrence.is_some_and(|occurrence| {
            occurrence.session != first.session || occurrence.capture_epoch != first.capture_epoch
        }) {
            return Err(io::Error::other(
                "event capture differs from trail envelope",
            ));
        }
        match &row.event {
            TrailEvent::Qualification {
                evidence,
                calibration,
            } => {
                if !ledger.qualify(evidence, calibration).is_qualified() {
                    return Err(io::Error::other("recorded qualification refused"));
                }
            }
            TrailEvent::Decision { decision } => {
                let input = decision
                    .input
                    .as_ref()
                    .ok_or_else(|| io::Error::other("unrecorded admission corridor"))?;
                if ledger
                    .slots_of(&decision.observation.occurrence)
                    .unwrap_or(&[])
                    != input.source_slots
                {
                    return Err(io::Error::other("replay source pins differ"));
                }
                if let Some(rate) = input.capture_rate_hz {
                    ledger.bind_capture_rate(rate);
                }
                let verdict = ledger.admit_with_slots(
                    &decision.observation,
                    &decision.candidate_label,
                    input.offered_slots.clone(),
                    input.slot_revision,
                );
                if verdict != decision.verdict
                    || ledger
                        .slots_of(&decision.observation.occurrence)
                        .unwrap_or(&[])
                        != decision.result_slots
                {
                    return Err(io::Error::other("replay decision differs"));
                }
                project(&ledger, &decision.observation, &verdict);
            }
            TrailEvent::End {
                dropped_records,
                persistence_complete,
                ..
            } => {
                if *dropped_records != 0 || !persistence_complete {
                    return Err(io::Error::other("incomplete trail"));
                }
            }
            TrailEvent::Start { .. } => {}
            TrailEvent::Frontier {
                occurrence,
                operation,
                producers,
            } => match operation.as_str() {
                "schedule" => {
                    ledger.schedule_frontier(occurrence.clone(), producers.iter().copied())
                }
                "observer" => {
                    for producer in producers {
                        ledger.schedule_observer(occurrence.clone(), *producer);
                    }
                }
                "return" => {
                    for producer in producers {
                        ledger.note_frontier_return(occurrence, *producer);
                    }
                }
                _ => return Err(io::Error::other("unrecorded frontier operation")),
            },
            TrailEvent::Projection { .. } => {
                return Err(io::Error::other(
                    "document revision replay needs its reducer action",
                ));
            }
        }
    }
    if !matches!(
        records.last().map(|row| &row.event),
        Some(TrailEvent::End {
            persistence_complete: true,
            dropped_records: 0,
            ..
        })
    ) {
        return Err(io::Error::other("trail lacks complete end"));
    }
    Ok(ledger)
}

#[cfg(test)]
mod tests {
    use super::super::acoustic_ledger::{ObservationProducer, WordPin};
    use super::*;

    #[test]
    fn persisted_five_pins_trace_and_production_admission_replay() {
        let dir = tempfile::tempdir().unwrap();
        let occurrence = OccurrenceIdentity::new("trail-five-iwo", 7, 0, 16_000);
        let sink = TrailSink::open_in(dir.path(), &occurrence.session, 7, 64).unwrap();
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("trail-test", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(0),
            vad_close_sample: Some(16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let pins = (0..5)
            .map(|i| WordPin::new(i * 3000, i * 3000 + 2000, "Iwo"))
            .collect::<Vec<_>>();
        ledger.admit_pinned_label(&apple, "Iwo Iwo Iwo Iwo Iwo", &pins);
        let lexicon =
            ObservationIdentity::new(ObservationProducer::Lexicon, 2, 0, occurrence.clone());
        ledger.admit(&lexicon, "Iwo Iwo Iwo Iwo Iwo");
        drop(sink);
        let rows = read_trail(&trail_path(dir.path(), &occurrence.session).unwrap()).unwrap();
        let trace = render_trace(&rows, Some("Iwo"));
        assert_eq!(
            trace
                .lines()
                .filter(|line| line.contains(" introduced:"))
                .count(),
            5
        );
        assert!(trace.contains("producer=lexicon"));
        assert!(trace.contains("retained_text-2-0-1"));
        let mut projected = Vec::new();
        let replayed = replay_decisions(&rows, |ledger, observation, receipt| {
            if receipt.grants_mutation() {
                projected.push(ledger.text_of(&observation.occurrence).unwrap().to_owned());
            }
        })
        .unwrap();
        assert_eq!(ledger.slots_of(&occurrence), replayed.slots_of(&occurrence));
        assert_eq!(projected, ["Iwo Iwo Iwo Iwo Iwo"]);
        assert!(replay_decisions(&rows[..rows.len() - 1], |_, _, _| {}).is_err());
        let mut mismatched = rows.clone();
        if let TrailEvent::Qualification { evidence, .. } = &mut mismatched[1].event {
            evidence.occurrence.capture_epoch += 1;
        }
        assert!(replay_decisions(&mismatched, |_, _, _| {}).is_err());
    }

    #[test]
    fn overflow_is_counted_and_missing_end_cannot_claim_completeness() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let occurrence = OccurrenceIdentity::new("trail-overflow", 9, 0, 10);
        let state = Arc::new(SenderState {
            sender: Mutex::new(Some(sender)),
            dropped: Arc::new(AtomicU64::new(0)),
            origin: Instant::now(),
        });
        let key = (occurrence.session.clone(), 9);
        SINKS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(key.clone(), Arc::downgrade(&state));
        for _ in 0..3 {
            record(
                &occurrence,
                TrailEvent::Start {
                    sample_clock: "capture_samples".into(),
                },
            );
        }
        assert_eq!(state.dropped.load(Ordering::Relaxed), 2);
        SINKS.get().unwrap().lock().unwrap().remove(&key);
        assert!(render_trace(&[], None).contains("INCOMPLETE"));
        assert!(trail_path(Path::new("/tmp"), "../escape").is_err());
    }
}
