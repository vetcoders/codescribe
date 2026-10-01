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
    AcousticEvidence, AcousticLedger, DictionarySlotRule, EnergyCalibration, LayerDecisionReceipt,
    MutationReceipt, ObservationIdentity, OccurrenceIdentity, SlotAlternative, SlotOperationKind,
    SlotOperationReceipt, SlotTarget, WordPin, WordSlot,
};

const SCHEMA: &str = "codescribe.decision-trail.v2";
const ADMISSION_SCHEMA: &str = "codescribe.decision-trail.v1";
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

/// Serialized production inputs; PCM geometry and confidence travel unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TrailSlotInput {
    Words {
        words: Vec<TrailWordPin>,
    },
    Rewrite {
        rewrites: Vec<TrailDictionaryRewrite>,
    },
    Merge {
        targets: Vec<TrailSlotTarget>,
        rule: TrailDictionaryRule,
    },
    Split {
        target: TrailSlotTarget,
        children: Vec<TrailWordPin>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailWordPin {
    pub sample_start: u64,
    pub sample_end: u64,
    pub text: String,
    pub confidence: Option<super::word_confidence::WordConfidence>,
    pub surface_rewritten: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode_sample_start: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailSlotTarget {
    pub observation: ObservationIdentity,
    pub sample_start: u64,
    pub sample_end: u64,
}

impl From<&SlotTarget> for TrailSlotTarget {
    fn from(target: &SlotTarget) -> Self {
        Self {
            observation: target.observation.clone(),
            sample_start: target.sample_start,
            sample_end: target.sample_end,
        }
    }
}

impl TrailSlotTarget {
    fn target(&self) -> SlotTarget {
        SlotTarget {
            observation: self.observation.clone(),
            sample_start: self.sample_start,
            sample_end: self.sample_end,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailDictionaryRule {
    pub id: String,
    pub input: Vec<String>,
    pub canonical: String,
}

impl From<&DictionarySlotRule> for TrailDictionaryRule {
    fn from(rule: &DictionarySlotRule) -> Self {
        Self {
            id: rule.id.clone(),
            input: rule.input.clone(),
            canonical: rule.canonical.clone(),
        }
    }
}

impl TrailDictionaryRule {
    fn rule(&self) -> DictionarySlotRule {
        DictionarySlotRule {
            id: self.id.clone(),
            input: self.input.clone(),
            canonical: self.canonical.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailDictionaryRewrite {
    pub target: TrailSlotTarget,
    pub rule: TrailDictionaryRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailSlotOperation {
    pub observation: ObservationIdentity,
    pub kind: String,
    pub sources: Vec<WordSlot>,
    pub source_ranges: Vec<OccurrenceIdentity>,
    pub outputs: Vec<WordSlot>,
    pub rule_id: String,
}

impl From<&SlotOperationReceipt> for TrailSlotOperation {
    fn from(receipt: &SlotOperationReceipt) -> Self {
        Self {
            observation: receipt.observation.clone(),
            kind: match receipt.kind {
                SlotOperationKind::Correct => "correct",
                SlotOperationKind::Insert => "insert",
                SlotOperationKind::Merge => "merge",
                SlotOperationKind::Split => "split",
                SlotOperationKind::Delete => "delete",
            }
            .into(),
            sources: receipt.sources.clone(),
            source_ranges: receipt.source_ranges.clone(),
            outputs: receipt.outputs.clone(),
            rule_id: receipt.rule_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailSlotAlternative {
    pub observation: ObservationIdentity,
    pub candidate: String,
    pub sources: Vec<WordSlot>,
    pub reason: String,
}

impl From<&SlotAlternative> for TrailSlotAlternative {
    fn from(alternative: &SlotAlternative) -> Self {
        Self {
            observation: alternative.observation.clone(),
            candidate: alternative.candidate.clone(),
            sources: alternative.sources.clone(),
            reason: alternative.reason.into(),
        }
    }
}

/// Acoustic measurements that authorized this particular word batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailSpeechEvidence {
    session: String,
    capture_epoch: u64,
    producer: String,
    observed_samples: Option<u64>,
    ranges: Vec<OccurrenceIdentity>,
}

impl TrailSpeechEvidence {
    fn from_evidence(speech: &crate::audio::capture_receipt::AcousticSpeechEvidence) -> Self {
        Self {
            session: speech.identity().session.clone(),
            capture_epoch: speech.identity().capture_epoch,
            producer: speech.producer().into(),
            observed_samples: speech.availability().observed_samples(),
            ranges: speech
                .ranges()
                .iter()
                .map(OccurrenceIdentity::from)
                .collect(),
        }
    }

    fn evidence(&self) -> io::Result<crate::audio::capture_receipt::AcousticSpeechEvidence> {
        use super::streaming::silero_fusion::SILERO_BOUNDARIES_PRODUCER;
        use crate::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CAPTURE_ENERGY_PRODUCER,
            CaptureEvidenceIdentity,
        };
        let producer = match self.producer.as_str() {
            CAPTURE_ENERGY_PRODUCER => CAPTURE_ENERGY_PRODUCER,
            SILERO_BOUNDARIES_PRODUCER => SILERO_BOUNDARIES_PRODUCER,
            _ => return Err(io::Error::other("unrecognized speech measurement producer")),
        };
        Ok(AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new(&self.session, self.capture_epoch),
            producer,
            self.observed_samples
                .map_or(AcousticAvailability::NotObserved, |observed_samples| {
                    AcousticAvailability::Observed { observed_samples }
                }),
            self.ranges
                .iter()
                .map(|range| crate::stt::tail_provider::TailSampleRange {
                    session: range.session.clone(),
                    capture_epoch: range.capture_epoch,
                    sample_start: range.sample_start,
                    sample_end: range.sample_end,
                })
                .collect(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailSlotStart {
    pub observation: ObservationIdentity,
    pub input: TrailSlotInput,
    pub source_slots: Vec<WordSlot>,
    pub capture_rate_hz: Option<u32>,
    pub first_ordinal: usize,
    pub operations_before: usize,
    pub alternatives_before: usize,
    #[serde(default)]
    pub speech: Option<TrailSpeechEvidence>,
    #[serde(default)]
    pub assignments: Vec<(OccurrenceIdentity, OccurrenceIdentity)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailSlotEnd {
    pub observation: ObservationIdentity,
    pub first_ordinal: usize,
    pub decisions: usize,
    pub result_slots: Vec<WordSlot>,
    pub operations: Vec<TrailSlotOperation>,
    pub alternatives: Vec<TrailSlotAlternative>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailDecisionEffects {
    pub ordinal: usize,
    pub operations: Vec<TrailSlotOperation>,
    pub alternatives: Vec<TrailSlotAlternative>,
}

fn decision_effects(ledger: &AcousticLedger, entry: &LayerDecisionReceipt) -> TrailDecisionEffects {
    TrailDecisionEffects {
        ordinal: entry.ordinal,
        operations: ledger
            .slot_operations()
            .iter()
            .filter(|operation| operation.observation == entry.observation)
            .map(Into::into)
            .collect(),
        alternatives: ledger
            .slot_alternatives()
            .iter()
            .filter(|alternative| alternative.observation == entry.observation)
            .map(Into::into)
            .collect(),
    }
}

/// Passive recording token. No ledger borrow, no admission authority.
/// An input without its completion is refused by replay, including on panic.
pub(super) struct SlotTrace(Option<TrailSlotStart>);

impl SlotTrace {
    fn begin(
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        rate: Option<u32>,
        input: impl FnOnce() -> TrailSlotInput,
    ) -> Self {
        if !is_enabled(&observation.occurrence) {
            return Self(None);
        }
        let start = TrailSlotStart {
            observation: observation.clone(),
            input: input(),
            source_slots: ledger
                .slots_of(&observation.occurrence)
                .unwrap_or(&[])
                .to_vec(),
            capture_rate_hz: rate,
            first_ordinal: ledger.layer_trail().len(),
            operations_before: ledger.slot_operations().len(),
            alternatives_before: ledger.slot_alternatives().len(),
            speech: ledger
                .speech_evidence
                .as_ref()
                .map(TrailSpeechEvidence::from_evidence),
            assignments: ledger.assigned_word_pin_ranges(observation),
        };
        record(
            &observation.occurrence,
            TrailEvent::SlotStart {
                operation: Box::new(start.clone()),
            },
        );
        Self(Some(start))
    }

    pub(super) fn words(
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        words: &[WordPin],
        rate: Option<u32>,
    ) -> Self {
        Self::begin(ledger, observation, rate, || TrailSlotInput::Words {
            words: saved_pins(words),
        })
    }

    pub(super) fn rewrite(
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        rewrites: &[(SlotTarget, DictionarySlotRule)],
    ) -> Self {
        Self::begin(ledger, observation, None, || TrailSlotInput::Rewrite {
            rewrites: rewrites
                .iter()
                .map(|(target, rule)| TrailDictionaryRewrite {
                    target: target.into(),
                    rule: rule.into(),
                })
                .collect(),
        })
    }

    pub(super) fn merge(
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        targets: &[SlotTarget],
        rule: &DictionarySlotRule,
    ) -> Self {
        Self::begin(ledger, observation, None, || TrailSlotInput::Merge {
            targets: targets.iter().map(Into::into).collect(),
            rule: rule.into(),
        })
    }

    pub(super) fn split(
        ledger: &AcousticLedger,
        observation: &ObservationIdentity,
        target: &SlotTarget,
        children: &[WordPin],
    ) -> Self {
        Self::begin(ledger, observation, None, || TrailSlotInput::Split {
            target: target.into(),
            children: saved_pins(children),
        })
    }

    pub(super) fn finish<T>(self, result: T, ledger: &AcousticLedger) -> T {
        if let Some(start) = self.0 {
            record(
                &start.observation.occurrence,
                TrailEvent::SlotEnd {
                    operation: Box::new(TrailSlotEnd {
                        observation: start.observation.clone(),
                        first_ordinal: start.first_ordinal,
                        decisions: ledger.layer_trail().len() - start.first_ordinal,
                        result_slots: ledger
                            .slots_of(&start.observation.occurrence)
                            .unwrap_or(&[])
                            .to_vec(),
                        operations: ledger.slot_operations()[start.operations_before..]
                            .iter()
                            .map(Into::into)
                            .collect(),
                        alternatives: ledger.slot_alternatives()[start.alternatives_before..]
                            .iter()
                            .map(Into::into)
                            .collect(),
                    }),
                },
            );
        }
        result
    }
}

fn saved_pins(pins: &[WordPin]) -> Vec<TrailWordPin> {
    pins.iter()
        .map(|pin| TrailWordPin {
            sample_start: pin.sample_start,
            sample_end: pin.sample_end,
            text: pin.text.clone(),
            confidence: pin.confidence,
            surface_rewritten: pin.surface_rewritten,
            decode_sample_start: pin.decode_sample_start,
        })
        .collect()
}

fn word_pins(slots: &[TrailWordPin]) -> Vec<WordPin> {
    slots
        .iter()
        .map(|slot| WordPin {
            sample_start: slot.sample_start,
            sample_end: slot.sample_end,
            text: slot.text.clone(),
            confidence: slot.confidence,
            surface_rewritten: slot.surface_rewritten,
            decode_sample_start: slot.decode_sample_start,
        })
        .collect()
}

// Capture exact intermediate receipts without cloning the entire ledger.
// Projection uses the completed operation, including its final source lineage.
struct ReplayCheckpoint {
    decision: TrailDecision,
    effects: TrailDecisionEffects,
}

thread_local! {
    static REPLAY_CHECKPOINTS: std::cell::RefCell<Option<Vec<ReplayCheckpoint>>> = const { std::cell::RefCell::new(None) };
}

struct ReplayCheckpoints;
impl Drop for ReplayCheckpoints {
    fn drop(&mut self) {
        REPLAY_CHECKPOINTS.with(|rows| rows.borrow_mut().take());
    }
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
    DecisionEffects {
        effects: Box<TrailDecisionEffects>,
    },
    Checkpoint {
        record_count: u64,
    },
    SlotStart {
        operation: Box<TrailSlotStart>,
    },
    SlotEnd {
        operation: Box<TrailSlotEnd>,
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
                let mut record_count = 0;
                for record in receiver {
                    last_ns = record.observed_ns;
                    write(&record)?;
                    record_count += 1;
                }
                let count = dropped.load(Ordering::Relaxed);
                write(&TrailRecord {
                    schema: SCHEMA.into(),
                    session: session.clone(),
                    capture_epoch: epoch,
                    observed_ns: last_ns,
                    event: TrailEvent::Checkpoint { record_count },
                })?;
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

fn decision_snapshot(
    ledger: &AcousticLedger,
    entry: &LayerDecisionReceipt,
    input: Option<TrailAdmission>,
) -> TrailDecision {
    TrailDecision {
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
    }
}

pub(super) fn decision(
    ledger: &AcousticLedger,
    entry: &LayerDecisionReceipt,
    input: Option<TrailAdmission>,
) {
    REPLAY_CHECKPOINTS.with(|rows| {
        if let Some(rows) = rows.borrow_mut().as_mut() {
            rows.push(ReplayCheckpoint {
                decision: decision_snapshot(ledger, entry, None),
                effects: decision_effects(ledger, entry),
            });
        }
    });
    if is_enabled(&entry.observation.occurrence) {
        record(
            &entry.observation.occurrence,
            TrailEvent::Decision {
                decision: Box::new(decision_snapshot(ledger, entry, input)),
            },
        );
        record(
            &entry.observation.occurrence,
            TrailEvent::DecisionEffects {
                effects: Box::new(decision_effects(ledger, entry)),
            },
        );
    }
}

pub fn read_trail(path: &Path) -> io::Result<Vec<TrailRecord>> {
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut records = Vec::new();
    let mut line = String::new();
    while reader.read_line(&mut line)? != 0 {
        let record: TrailRecord = serde_json::from_str(&line).map_err(io::Error::other)?;
        if record.schema == SCHEMA && !line.ends_with('\n') {
            return Err(io::Error::other("truncated decision trail line"));
        }
        if record.schema != SCHEMA && record.schema != ADMISSION_SCHEMA {
            return Err(io::Error::other("unsupported decision trail schema"));
        }
        records.push(record);
        line.clear();
    }
    Ok(records)
}

/// Per-pin fate, keyed by PCM geometry. Identical words remain separate rows.
/// Labels without word geometry are explicitly unpinned diagnostic hypotheses.
pub fn render_trace(records: &[TrailRecord], word: Option<&str>) -> String {
    let mut output = String::new();
    let complete = validate_envelope(records).is_ok();
    let mut slot_input: Option<&TrailSlotStart> = None;
    output.push_str(if complete {
        "Persistence complete; N2/revision evidence incomplete.\n"
    } else {
        "INCOMPLETE TRAIL: missing end, overflow or write failure.\n"
    });
    for row in records {
        match &row.event {
            TrailEvent::SlotStart { operation } => slot_input = Some(operation),
            TrailEvent::SlotEnd { .. } => slot_input = None,
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
            .or_else(|| {
                slot_input
                    .filter(|input| input.observation == decision.observation)
                    .map(|input| input.source_slots.as_slice())
            })
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

/// Replay production inputs only after the entire saved history validates.
/// No projection callback runs for a truncated or divergent history.
pub fn replay_decisions(
    records: &[TrailRecord],
    project: impl FnMut(&AcousticLedger, &ObservationIdentity, &MutationReceipt),
) -> io::Result<AcousticLedger> {
    validate_envelope(records)?;
    replay_validated(records, |_, _, _| {})?;
    replay_validated(records, project)
}

fn validate_envelope(records: &[TrailRecord]) -> io::Result<()> {
    let first = records
        .first()
        .ok_or_else(|| io::Error::other("empty trail"))?;
    if !matches!(first.event, TrailEvent::Start { .. }) {
        return Err(io::Error::other("trail lacks start"));
    }
    let Some(TrailEvent::End {
        dropped_records: 0,
        persistence_complete: true,
        ..
    }) = records.last().map(|row| &row.event)
    else {
        return Err(io::Error::other("trail lacks complete end"));
    };
    if first.schema == SCHEMA {
        if !matches!(records.get(records.len().saturating_sub(2)).map(|row| &row.event),
            Some(TrailEvent::Checkpoint { record_count }) if *record_count == records.len().saturating_sub(3) as u64)
        {
            return Err(io::Error::other("incomplete trail: record count differs"));
        }
    } else if first.schema != ADMISSION_SCHEMA {
        return Err(io::Error::other("unsupported decision trail schema"));
    }
    let identity = OccurrenceIdentity::new(first.session.clone(), first.capture_epoch, 0, 1);
    if is_enabled(&identity) {
        return Err(io::Error::other(
            "cannot replay into an active trail session",
        ));
    }
    let mut prior_ns = 0;
    for (index, row) in records.iter().enumerate() {
        if row.schema != first.schema
            || row.session != first.session
            || row.capture_epoch != first.capture_epoch
        {
            return Err(io::Error::other("trail identity or schema differs"));
        }
        if row.observed_ns < prior_ns {
            return Err(io::Error::other("trail clock regressed"));
        }
        prior_ns = row.observed_ns;
        if (matches!(row.event, TrailEvent::Start { .. }) && index != 0)
            || (matches!(row.event, TrailEvent::End { .. }) && index + 1 != records.len())
            || (matches!(row.event, TrailEvent::Checkpoint { .. }) && index + 2 != records.len())
        {
            return Err(io::Error::other("trail has interior start or end"));
        }
        let occurrence = match &row.event {
            TrailEvent::Qualification { evidence, .. } => Some(&evidence.occurrence),
            TrailEvent::Decision { decision } => Some(&decision.observation.occurrence),
            TrailEvent::Frontier { occurrence, .. } => Some(occurrence),
            TrailEvent::SlotStart { operation } => Some(&operation.observation.occurrence),
            TrailEvent::SlotEnd { operation } => Some(&operation.observation.occurrence),
            _ => None,
        };
        if occurrence.is_some_and(|occurrence| !occurrence.same_capture(&identity)) {
            return Err(io::Error::other(
                "event capture differs from trail envelope",
            ));
        }
    }
    Ok(())
}

fn decision_matches(actual: &TrailDecision, expected: &TrailDecision) -> bool {
    actual.ordinal == expected.ordinal
        && actual.receipt_id == expected.receipt_id
        && actual.producer == expected.producer
        && actual.layer == expected.layer
        && actual.observation == expected.observation
        && actual.candidate_label == expected.candidate_label
        && actual.candidate_tokens == expected.candidate_tokens
        && actual.verdict == expected.verdict
        && actual.predecessor_ordinal == expected.predecessor_ordinal
        && actual.result_slots == expected.result_slots
}

fn effects_match(ledger: &AcousticLedger, expected: &TrailDecisionEffects) -> bool {
    let Some(entry) = ledger.layer_trail().last() else {
        return false;
    };
    let actual = decision_effects(ledger, entry);
    actual == *expected
}

fn replay_slot_operation(
    ledger: &mut AcousticLedger,
    start: &TrailSlotStart,
    end: &TrailSlotEnd,
    decisions: &[&TrailDecision],
    effects: &[&TrailDecisionEffects],
    project: &mut impl FnMut(&AcousticLedger, &ObservationIdentity, &MutationReceipt),
) -> io::Result<()> {
    if start.observation != end.observation
        || start.first_ordinal != end.first_ordinal
        || start.first_ordinal != ledger.layer_trail().len()
        || end.decisions != decisions.len()
        || start.operations_before != ledger.slot_operations().len()
        || start.alternatives_before != ledger.slot_alternatives().len()
        || start.source_slots
            != ledger
                .slots_of(&start.observation.occurrence)
                .unwrap_or(&[])
    {
        return Err(io::Error::other(
            "slot operation source or decision range differs",
        ));
    }
    if let Some(rate) = start.capture_rate_hz {
        ledger.bind_capture_rate(rate);
    }
    REPLAY_CHECKPOINTS.with(|rows| *rows.borrow_mut() = Some(Vec::new()));
    let _checkpoints = ReplayCheckpoints;
    let observation = &start.observation;
    ledger.speech_evidence = start
        .speech
        .as_ref()
        .map(TrailSpeechEvidence::evidence)
        .transpose()?;
    ledger.record_assigned_word_pins(observation, &start.assignments);
    match &start.input {
        TrailSlotInput::Words { words } => {
            ledger.admit_word_slots(observation, &word_pins(words));
        }
        TrailSlotInput::Rewrite { rewrites } => {
            let rewrites = rewrites
                .iter()
                .map(|rewrite| (rewrite.target.target(), rewrite.rule.rule()))
                .collect::<Vec<_>>();
            ledger
                .rewrite_dictionary_slots(observation, &rewrites)
                .map_err(|reason| {
                    io::Error::other(format!("recorded dictionary rewrite refused: {reason:?}"))
                })?;
        }
        TrailSlotInput::Merge { targets, rule } => {
            let targets = targets
                .iter()
                .map(TrailSlotTarget::target)
                .collect::<Vec<_>>();
            ledger
                .merge_word_slots(observation, &targets, &rule.rule())
                .map_err(|reason| {
                    io::Error::other(format!("recorded dictionary merge refused: {reason:?}"))
                })?;
        }
        TrailSlotInput::Split { target, children } => {
            ledger
                .split_word_slot(observation, &target.target(), &word_pins(children))
                .map_err(|reason| {
                    io::Error::other(format!("recorded split refused: {reason:?}"))
                })?;
        }
    }
    let checkpoints = REPLAY_CHECKPOINTS.with(|rows| rows.borrow_mut().take().unwrap_or_default());
    let operations = ledger.slot_operations()[start.operations_before..]
        .iter()
        .map(TrailSlotOperation::from)
        .collect::<Vec<_>>();
    let alternatives = ledger.slot_alternatives()[start.alternatives_before..]
        .iter()
        .map(TrailSlotAlternative::from)
        .collect::<Vec<_>>();
    if checkpoints.len() != decisions.len()
        || effects.len() != decisions.len()
        || !checkpoints
            .iter()
            .zip(effects)
            .all(|(checkpoint, expected)| checkpoint.effects == **expected)
        || !checkpoints
            .iter()
            .zip(decisions)
            .all(|(checkpoint, expected)| decision_matches(&checkpoint.decision, expected))
        || ledger.slots_of(&observation.occurrence).unwrap_or(&[]) != end.result_slots
        || operations != end.operations
        || alternatives != end.alternatives
    {
        return Err(io::Error::other(
            "replay slot operation, lineage or decisions differ",
        ));
    }
    for checkpoint in checkpoints {
        project(
            ledger,
            &checkpoint.decision.observation,
            &checkpoint.decision.verdict,
        );
    }
    Ok(())
}

struct PendingSlotReplay<'a> {
    start: &'a TrailSlotStart,
    decisions: Vec<&'a TrailDecision>,
    effects: Vec<&'a TrailDecisionEffects>,
}

fn replay_validated(
    records: &[TrailRecord],
    mut project: impl FnMut(&AcousticLedger, &ObservationIdentity, &MutationReceipt),
) -> io::Result<AcousticLedger> {
    let mut ledger = AcousticLedger::new();
    let mut pending: Option<PendingSlotReplay<'_>> = None;
    let mut effects_due = false;
    for row in records {
        if effects_due && !matches!(row.event, TrailEvent::DecisionEffects { .. }) {
            return Err(io::Error::other("decision lacks slot effects"));
        }
        match &row.event {
            TrailEvent::SlotStart { operation } => {
                if pending.is_some() {
                    return Err(io::Error::other("nested slot input"));
                }
                pending = Some(PendingSlotReplay {
                    start: operation,
                    decisions: Vec::new(),
                    effects: Vec::new(),
                });
            }
            TrailEvent::Decision { decision } => {
                effects_due = row.schema == SCHEMA;
                if let Some(pending) = &mut pending {
                    pending.decisions.push(decision);
                    continue;
                }
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
                let actual = ledger
                    .layer_trail()
                    .last()
                    .ok_or_else(|| io::Error::other("replay decision missing"))?;
                if !decision_matches(&decision_snapshot(&ledger, actual, None), decision) {
                    return Err(io::Error::other("replay decision differs"));
                }
                project(&ledger, &decision.observation, &verdict);
            }
            TrailEvent::DecisionEffects { effects } => {
                if !effects_due {
                    return Err(io::Error::other("slot effects lack decision"));
                }
                effects_due = false;
                if let Some(pending) = &mut pending {
                    pending.effects.push(effects);
                } else if !effects_match(&ledger, effects) {
                    return Err(io::Error::other("replay decision effects differ"));
                }
            }
            TrailEvent::SlotEnd { operation } => {
                let pending = pending
                    .take()
                    .ok_or_else(|| io::Error::other("slot completion lacks input"))?;
                replay_slot_operation(
                    &mut ledger,
                    pending.start,
                    operation,
                    &pending.decisions,
                    &pending.effects,
                    &mut project,
                )?;
            }
            _ if pending.is_some() => return Err(io::Error::other("incomplete slot operation")),
            TrailEvent::Qualification {
                evidence,
                calibration,
            } => {
                if !ledger.qualify(evidence, calibration).is_qualified() {
                    return Err(io::Error::other("recorded qualification refused"));
                }
            }
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
            TrailEvent::Start { .. } | TrailEvent::End { .. } | TrailEvent::Checkpoint { .. } => {}
        }
    }
    if pending.is_some() {
        return Err(io::Error::other("slot operation lacks completion"));
    }
    Ok(ledger)
}

#[cfg(test)]
mod tests {
    use super::super::acoustic_ledger::{ObservationProducer, WordPin};
    use super::*;

    fn saved_slot_scenario(
        scenario: &str,
    ) -> (
        tempfile::TempDir,
        OccurrenceIdentity,
        AcousticLedger,
        Vec<TrailRecord>,
    ) {
        use super::super::word_confidence::{WordConfidence, WordConfidenceSource};
        let dir = tempfile::tempdir().unwrap();
        let owner = OccurrenceIdentity::new(format!("slot-trail-{scenario}"), 11, 0, 16_000);
        let sink = TrailSink::open_in(dir.path(), &owner.session, 11, 128).unwrap();
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(16_000);
        let calibration = EnergyCalibration::new("slot-trail", 1.0, 1);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 1_000.0,
                        energy_integral: 10.0,
                        mean_rms_dbfs: -12.0,
                        peak_dbfs: -3.0,
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(16_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
        let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, owner.clone());
        let children = [
            WordPin::new(0, 4_000, "na").with_confidence(WordConfidence::new(
                WordConfidenceSource::WhisperTokenLogprob,
                -0.25,
                2,
            )),
            WordPin::new(8_000, 16_000, "prawdę").surface_rewritten(),
        ];
        match scenario {
            "batch" => {
                ledger.admit_word_slots(&apple, &[WordPin::new(0, 4_000, "no")]);
                ledger.admit_word_slots(&whisper, &children);
                assert_eq!(ledger.text_of(&owner), Some("na prawdę"));
                assert!(
                    ledger
                        .slot_operations()
                        .iter()
                        .any(|op| op.kind == SlotOperationKind::Correct)
                );
                assert!(
                    ledger
                        .slot_operations()
                        .iter()
                        .any(|op| op.kind == SlotOperationKind::Insert)
                );
                // This candidate cannot pick one of the two physical sources.
                let ambiguous =
                    ObservationIdentity::new(ObservationProducer::Whisper, 3, 2, owner.clone());
                ledger.admit_word_slots(&ambiguous, &[WordPin::new(0, 16_000, "naprawdę")]);
                assert!(!ledger.slot_alternatives().is_empty());
                // Also retain a whole-label alternative via the existing corridor.
                ledger.admit(
                    &ObservationIdentity::new(ObservationProducer::Lexicon, 4, 2, owner.clone()),
                    "other",
                );
            }
            "speech-resegment" => {
                use crate::audio::capture_receipt::{
                    AcousticAvailability, AcousticSpeechEvidence, CAPTURE_ENERGY_PRODUCER,
                    CaptureEvidenceIdentity,
                };
                ledger.admit_word_slots(
                    &apple,
                    &[
                        WordPin::new(0, 12_000, "apple floor"),
                        WordPin::new(14_000, 16_000, "dalej"),
                    ],
                );
                ledger.record_speech_evidence(&AcousticSpeechEvidence::measured(
                    CaptureEvidenceIdentity::new(&owner.session, 11),
                    CAPTURE_ENERGY_PRODUCER,
                    AcousticAvailability::Observed {
                        observed_samples: 16_000,
                    },
                    vec![crate::stt::tail_provider::TailSampleRange {
                        session: owner.session.clone(),
                        capture_epoch: 11,
                        sample_start: 1_000,
                        sample_end: 11_000,
                    }],
                ));
                ledger.admit_word_slots(
                    &whisper,
                    &[
                        WordPin::new(0, 6_000, "był"),
                        WordPin::new(6_000, 12_000, "sobie"),
                    ],
                );
                assert_eq!(ledger.text_of(&owner), Some("był sobie dalej"));
            }
            "group-split" => {
                ledger.admit(&apple, "na prawdę");
                ledger.admit_word_slots(&whisper, &children);
                assert_eq!(
                    ledger.slot_operations().last().unwrap().kind,
                    SlotOperationKind::Split
                );
            }
            "explicit-split" => {
                ledger.admit_word_slots(&apple, &[WordPin::new(0, 16_000, "na prawdę")]);
                let target = SlotTarget::from(&ledger.slots_of(&owner).unwrap()[0]);
                ledger
                    .split_word_slot(&whisper, &target, &children)
                    .unwrap();
                assert_eq!(
                    ledger.slot_operations().last().unwrap().kind,
                    SlotOperationKind::Split
                );
            }
            "rewrite-merge" => {
                ledger.admit_pinned_label(&apple, "na prawdę", &children);
                let target = SlotTarget::from(&ledger.slots_of(&owner).unwrap()[0]);
                let lexicon =
                    ObservationIdentity::new(ObservationProducer::Lexicon, 3, 1, owner.clone());
                ledger
                    .rewrite_dictionary_slots(
                        &lexicon,
                        &[(
                            target,
                            DictionarySlotRule {
                                id: "capitalization/v1".into(),
                                input: vec!["na".into()],
                                canonical: "Na".into(),
                            },
                        )],
                    )
                    .unwrap();
                let targets = ledger
                    .slots_of(&owner)
                    .unwrap()
                    .iter()
                    .map(SlotTarget::from)
                    .collect::<Vec<_>>();
                ledger
                    .merge_word_slots(
                        &ObservationIdentity::new(
                            ObservationProducer::Lexicon,
                            4,
                            2,
                            owner.clone(),
                        ),
                        &targets,
                        &DictionarySlotRule {
                            id: "merge/v1".into(),
                            input: vec!["Na".into(), "prawdę".into()],
                            canonical: "Naprawdę".into(),
                        },
                    )
                    .unwrap();
                assert_eq!(
                    ledger.slot_operations().last().unwrap().source_ranges,
                    [
                        OccurrenceIdentity::new(&owner.session, 11, 0, 4_000),
                        OccurrenceIdentity::new(&owner.session, 11, 8_000, 16_000),
                    ]
                );
            }
            _ => unreachable!(),
        }
        drop(sink);
        let rows = read_trail(&trail_path(dir.path(), &owner.session).unwrap()).unwrap();
        (dir, owner, ledger, rows)
    }

    #[test]
    fn saved_inputs_replay_insert_split_rewrite_and_alternatives_literally() {
        for scenario in [
            "speech-resegment",
            "batch",
            "group-split",
            "explicit-split",
            "rewrite-merge",
        ] {
            let (_dir, owner, ledger, rows) = saved_slot_scenario(scenario);
            let mut receipts = Vec::new();
            let mut merge_projection_ranges = Vec::new();
            let replayed = replay_decisions(&rows, |current, observation, receipt| {
                receipts.push((observation.clone(), receipt.clone()));
                if scenario == "rewrite-merge"
                    && observation.request == 4
                    && receipt.grants_mutation()
                {
                    merge_projection_ranges =
                        current.slot_source_ranges(&current.slots_of(&owner).unwrap()[0]);
                    assert_eq!(current.conservation().residue(), 0);
                }
            })
            .unwrap();
            assert_eq!(
                replayed.slots_of(&owner),
                ledger.slots_of(&owner),
                "{scenario}"
            );
            assert_eq!(
                replayed.slot_operations(),
                ledger.slot_operations(),
                "{scenario}"
            );
            assert_eq!(
                replayed.slot_alternatives(),
                ledger.slot_alternatives(),
                "{scenario}"
            );
            assert_eq!(replayed.layer_trail(), ledger.layer_trail(), "{scenario}");
            if scenario == "rewrite-merge" {
                assert_eq!(
                    merge_projection_ranges,
                    ledger.slot_source_ranges(&ledger.slots_of(&owner).unwrap()[0])
                );
            }
            assert_eq!(
                receipts,
                ledger
                    .layer_trail()
                    .iter()
                    .map(|entry| (entry.observation.clone(), entry.decision.clone()))
                    .collect::<Vec<_>>()
            );
        }
    }

    fn assert_replay_refused_before_projection(rows: &[TrailRecord]) {
        let mut projected = 0;
        assert!(replay_decisions(rows, |_, _, _| projected += 1).is_err());
        assert_eq!(
            projected, 0,
            "invalid history must not partially mutate the reducer"
        );
    }

    #[test]
    fn damaged_disk_history_and_copied_text_cannot_replay() {
        let (dir, owner, _ledger, rows) = saved_slot_scenario("rewrite-merge");
        for length in 0..rows.len() {
            assert_replay_refused_before_projection(&rows[..length]);
        }
        let mut missing_middle = rows.clone();
        missing_middle.remove(2);
        assert_replay_refused_before_projection(&missing_middle);
        let mut altered_lineage = rows.clone();
        let end = altered_lineage
            .iter_mut()
            .rev()
            .find_map(|row| match &mut row.event {
                TrailEvent::SlotEnd { operation } => Some(operation),
                _ => None,
            })
            .unwrap();
        // All final text is intact; only the real gap in source PCM is forged.
        end.operations[0].source_ranges[0].sample_end = 8_000;
        assert_replay_refused_before_projection(&altered_lineage);
        let mut text_only = rows.clone();
        for row in &mut text_only {
            if let TrailEvent::SlotStart { operation } = &mut row.event
                && matches!(operation.input, TrailSlotInput::Merge { .. })
            {
                operation.input = TrailSlotInput::Words {
                    words: saved_pins(&[WordPin::new(0, 16_000, "Naprawdę")]),
                };
            }
        }
        assert_replay_refused_before_projection(&text_only);
        let path = trail_path(dir.path(), &owner.session).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert!(
            read_trail(&path)
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
        let mut missing_input = rows.clone();
        for row in &mut missing_input {
            if let TrailEvent::Decision { decision } = &mut row.event {
                decision.input = None;
                break;
            }
        }
        assert_replay_refused_before_projection(&missing_input);
    }

    fn assert_relay_operation_replays(merge: bool) {
        use super::super::acoustic_ledger::{DictionarySlotRule, SlotTarget};
        let dir = tempfile::tempdir().unwrap();
        let session = if merge {
            "relay-trail-merge"
        } else {
            "relay-trail-correction"
        };
        let owner = OccurrenceIdentity::new(session, 7, 0, 16_000);
        let sink = TrailSink::open_in(dir.path(), session, 7, 64).unwrap();
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("relay-trail", 1.0, 1);
        assert!(
            ledger
                .qualify(
                    &AcousticEvidence {
                        occurrence: owner.clone(),
                        duration_ms: 1_000.0,
                        energy_integral: 10.0,
                        mean_rms_dbfs: -12.0,
                        peak_dbfs: -3.0,
                        vad_open_sample: Some(0),
                        vad_close_sample: Some(16_000),
                        evidence_calibration_version: calibration.version.clone(),
                    },
                    &calibration
                )
                .is_qualified()
        );
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
        if merge {
            ledger.admit_pinned_label(
                &apple,
                "na prawdę",
                &[
                    WordPin::new(0, 4_000, "na"),
                    WordPin::new(8_000, 16_000, "prawdę"),
                ],
            );
            let targets = ledger
                .slots_of(&owner)
                .unwrap()
                .iter()
                .map(SlotTarget::from)
                .collect::<Vec<_>>();
            ledger
                .merge_word_slots(
                    &ObservationIdentity::new(ObservationProducer::Lexicon, 2, 1, owner.clone()),
                    &targets,
                    &DictionarySlotRule {
                        id: "relay-test/na-prawde/v1".into(),
                        input: vec!["na".into(), "prawdę".into()],
                        canonical: "naprawdę".into(),
                    },
                )
                .unwrap();
            assert_eq!(ledger.text_of(&owner), Some("naprawdę"));
        } else {
            ledger.admit_pinned_label(
                &apple,
                "weryfikowałeś",
                &[WordPin::new(0, 16_000, "weryfikowałeś")],
            );
            ledger.admit_pinned_label(
                &ObservationIdentity::new(ObservationProducer::Whisper, 2, 1, owner.clone()),
                "zweryfikowałeś",
                &[],
            );
            assert_eq!(ledger.text_of(&owner), Some("zweryfikowałeś"));
        }
        assert!(
            !ledger.slot_operations().is_empty(),
            "exercise a real operation"
        );
        drop(sink);
        let rows = read_trail(&trail_path(dir.path(), session).unwrap()).unwrap();
        let replayed = replay_decisions(&rows, |_, _, _| {})
            .expect("a persisted accepted operation must replay without audio or a model");
        assert_eq!(replayed.slots_of(&owner), ledger.slots_of(&owner));
        assert_eq!(replayed.slot_operations(), ledger.slot_operations());
        for slot in ledger.slots_of(&owner).unwrap() {
            assert_eq!(
                replayed.slot_source_ranges(slot),
                ledger.slot_source_ranges(slot)
            );
        }
        assert!(replay_decisions(&rows[..rows.len() - 1], |_, _, _| {}).is_err());
    }

    #[test]
    fn relay_acceptance_trail_replays_acoustic_correction_and_lineage() {
        assert_relay_operation_replays(false);
    }

    #[test]
    fn relay_acceptance_trail_replays_dictionary_merge_with_disjoint_sources() {
        assert_relay_operation_replays(true);
    }

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
