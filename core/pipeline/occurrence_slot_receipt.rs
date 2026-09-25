//! Seal-time observations only. No receipt is fed back into admission, the Bus,
//! formatter, or agent context. The session owns a local sidecar sink; an
//! unregistered session (including ordinary ledger unit tests) does no IO.
//!
//! `survived` describes the FINAL slot geometry, not historical causation.
//! A slot can have several reasons against one Whisper observation: a midpoint
//! in a gap may also have no overlap at all. Deltas are slot minus Whisper on
//! the capture clock. Normalization matches `same_word_pin` (edge non-alphanumeric
//! characters stripped, Unicode lowercase), and never influences the ledger.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use super::acoustic_ledger::{
    AcousticLedger, ObservationIdentity, ObservationProducer, OccurrenceIdentity, WordSlot,
};
use crate::config::Config;

type CaptureKey = (String, u64);
static SINKS: OnceLock<Mutex<BTreeMap<CaptureKey, Arc<Sidecar>>>> = OnceLock::new();

struct Sidecar {
    sample_rate_hz: u32,
    // A failed write disables this sink, so a partial line is never extended by
    // another receipt. Failure is diagnostic and cannot refuse a ledger seal.
    file: Mutex<Option<File>>,
}

/// The session's lifetime token. Drop unregisters on success, error, or unwind.
/// This registry routes IO by capture identity; it owns no transcript state.
pub(crate) struct SlotReceiptSink {
    key: CaptureKey,
}

impl SlotReceiptSink {
    pub(crate) fn for_session(session: &str, epoch: u64, rate: u32) -> Option<Self> {
        match Self::open_in(&Config::config_dir(), session, epoch, rate) {
            Ok(sink) => Some(sink),
            Err(error) => {
                // No spoken text or filesystem path enters logs.
                tracing::warn!(kind = ?error.kind(), "slot receipt sidecar unavailable");
                None
            }
        }
    }

    fn open_in(root: &Path, session: &str, epoch: u64, rate: u32) -> io::Result<Self> {
        if rate == 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let path = sidecar_path(root, session)?;
        let key = (session.to_owned(), epoch);
        let mut sinks = SINKS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| io::Error::other("slot receipt registry poisoned"))?;
        if sinks.contains_key(&key) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        fs::create_dir_all(root.join("sessions"))?;
        let mut options = OpenOptions::new();
        // Never overwrite an earlier take or follow an existing file symlink.
        // A reused take ID disables diagnostics, not transcription.
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        sinks.insert(
            key.clone(),
            Arc::new(Sidecar {
                sample_rate_hz: rate,
                file: Mutex::new(Some(file)),
            }),
        );
        Ok(Self { key })
    }
}

impl Drop for SlotReceiptSink {
    fn drop(&mut self) {
        if let Some(sinks) = SINKS.get()
            && let Ok(mut sinks) = sinks.lock()
        {
            sinks.remove(&self.key);
        }
    }
}

/// `root` is resolved once by Config::config_dir at session entry. The filename
/// uses the same safe session alphabet as the retained session WAV.
fn sidecar_path(root: &Path, session: &str) -> io::Result<PathBuf> {
    if !(8..=80).contains(&session.len())
        || !session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    Ok(root.join("sessions").join(format!("{session}.slots.jsonl")))
}

/// Called exactly once in the new-seal branch, after the ledger stored its seal.
/// Only shared references cross this boundary; no diagnostic result is returned.
pub(super) fn emit(ledger: &AcousticLedger, occurrence: &OccurrenceIdentity) {
    let Some(sinks) = SINKS.get() else {
        return;
    };
    let sink = sinks.lock().ok().and_then(|sinks| {
        sinks
            .get(&(occurrence.session.clone(), occurrence.capture_epoch))
            .cloned()
    });
    let Some(sink) = sink else {
        return;
    };
    let Some(slots) = ledger.slots_of(occurrence) else {
        return;
    };
    let Ok(mut file) = sink.file.lock() else {
        return;
    };
    let Some(writer) = file.as_mut() else {
        return;
    };
    let receipt = build_receipt(occurrence, slots, sink.sample_rate_hz);
    let result = serde_json::to_vec(&receipt)
        .map_err(io::Error::other)
        .and_then(|mut bytes| {
            bytes.push(b'\n');
            writer.write_all(&bytes)
        });
    if let Err(error) = result {
        *file = None;
        tracing::warn!(kind = ?error.kind(), "slot receipt sidecar disabled after write failure");
    }
}

#[derive(Debug, Serialize)]
struct OccurrenceSlotReceipt<'a> {
    schema: &'static str,
    session: &'a str,
    capture_epoch: u64,
    // session + epoch + this range are the complete OccurrenceIdentity.
    sample_start: u64,
    sample_end: u64,
    sample_rate_hz: u32,
    slots: Vec<SlotReceipt<'a>>,
    whisper_cross_window_overlap: Vec<CrossWindowOverlap<'a>>,
}

#[derive(Debug, Serialize)]
struct ObservationReceipt {
    producer: &'static str,
    request: u64,
    generation: u64,
}

impl From<&ObservationIdentity> for ObservationReceipt {
    fn from(observation: &ObservationIdentity) -> Self {
        Self {
            producer: observation.producer.as_str(),
            request: observation.request,
            generation: observation.generation,
        }
    }
}

#[derive(Debug, Serialize)]
struct SlotReceipt<'a> {
    producer: &'static str,
    sample_start: u64,
    sample_end: u64,
    text: &'a str,
    observation: ObservationReceipt,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    survived: Vec<SurvivalEvidence<'a>>,
}

#[derive(Debug, Serialize)]
struct SurvivalEvidence<'a> {
    reasons: Vec<&'static str>,
    whisper_observation: ObservationReceipt,
    heard_sample_start: u64,
    heard_sample_end: u64,
    #[serde(flatten)]
    comparison: WordComparison<'a>,
}

#[derive(Debug, Serialize)]
struct WordComparison<'a> {
    nearest_whisper_word: WhisperWord<'a>,
    start_delta_ms: f64,
    end_delta_ms: f64,
    same_normalized_text: bool,
}

#[derive(Debug, Serialize)]
struct WhisperWord<'a> {
    slot_index: usize,
    text: &'a str,
    sample_start: u64,
    sample_end: u64,
}

#[derive(Debug, Serialize)]
struct CrossWindowOverlap<'a> {
    reason: &'static str,
    slot_index: usize,
    // The counterpart's index also resolves its observation in `slots`.
    #[serde(flatten)]
    comparison: WordComparison<'a>,
}

fn normalized(text: &str) -> String {
    text.trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn midpoint(slot: &WordSlot) -> u64 {
    slot.sample_start
        .saturating_add(slot.sample_end.saturating_sub(slot.sample_start) / 2)
}

fn overlaps(left: &WordSlot, right: &WordSlot) -> bool {
    left.sample_start < right.sample_end && right.sample_start < left.sample_end
}

fn comparison<'a>(
    slot: &WordSlot,
    whisper_index: usize,
    whisper: &'a WordSlot,
    rate: u32,
) -> WordComparison<'a> {
    // Subtract in signed integer space first, retaining sub-ms offsets even
    // when capture coordinates are large. Never assume a 16 kHz clock.
    let delta_ms = |sample: u64, reference: u64| {
        (i128::from(sample) - i128::from(reference)) as f64 * 1000.0 / f64::from(rate)
    };
    WordComparison {
        nearest_whisper_word: WhisperWord {
            slot_index: whisper_index,
            text: &whisper.text,
            sample_start: whisper.sample_start,
            sample_end: whisper.sample_end,
        },
        start_delta_ms: delta_ms(slot.sample_start, whisper.sample_start),
        end_delta_ms: delta_ms(slot.sample_end, whisper.sample_end),
        same_normalized_text: normalized(&slot.text) == normalized(&whisper.text),
    }
}

/// Classify each non-Whisper slot against EACH observation's span. A slot is
/// inside a span when the half-open ranges intersect (edge straddles included).
/// Reasons are non-exclusive. Midpoint-inside is recorded too: final geometry
/// alone cannot prove which admission order or later text shaping preserved it.
fn classify_survival<'a>(
    slot: &WordSlot,
    words: &[(usize, &'a WordSlot)],
    rate: u32,
) -> Option<SurvivalEvidence<'a>> {
    let heard_start = words.iter().map(|(_, word)| word.sample_start).min()?;
    let heard_end = words.iter().map(|(_, word)| word.sample_end).max()?;
    if slot.sample_start >= heard_end || slot.sample_end <= heard_start {
        return None;
    }
    let mid = midpoint(slot);
    let midpoint_inside = words
        .iter()
        .any(|(_, word)| word.sample_start <= mid && mid < word.sample_end);
    let any_overlap = words.iter().any(|(_, word)| overlaps(slot, word));
    let mut reasons = Vec::new();
    if !midpoint_inside && heard_start <= mid && mid < heard_end {
        reasons.push("midpoint_in_whisper_gap");
    }
    if !any_overlap {
        reasons.push("no_whisper_overlap");
    } else if !midpoint_inside {
        reasons.push("partial_overlap_midpoint_outside");
    } else {
        reasons.push("midpoint_inside_whisper_word");
    }
    // Nearest by interval distance, then midpoint distance, then final slot
    // index. Text never biases selection towards the hypothesized double.
    let &(nearest_index, nearest) = words.iter().min_by_key(|(index, word)| {
        let distance = word
            .sample_start
            .saturating_sub(slot.sample_end)
            .max(slot.sample_start.saturating_sub(word.sample_end));
        (distance, mid.abs_diff(midpoint(word)), *index)
    })?;
    Some(SurvivalEvidence {
        reasons,
        whisper_observation: ObservationReceipt::from(&nearest.observation),
        heard_sample_start: heard_start,
        heard_sample_end: heard_end,
        comparison: comparison(slot, nearest_index, nearest, rate),
    })
}

fn build_receipt<'a>(
    occurrence: &'a OccurrenceIdentity,
    slots: &'a [WordSlot],
    rate: u32,
) -> OccurrenceSlotReceipt<'a> {
    // Group only this occurrence's final Whisper slots by complete observation
    // identity. BTree order makes bytes stable regardless of hash seed.
    let mut windows: BTreeMap<(u64, u64), Vec<(usize, &WordSlot)>> = BTreeMap::new();
    for (index, slot) in slots.iter().enumerate() {
        if slot.producer == ObservationProducer::Whisper {
            windows
                .entry((slot.observation.request, slot.observation.generation))
                .or_default()
                .push((index, slot));
        }
    }
    let mut cross_window = Vec::new();
    for (index, left) in slots.iter().enumerate() {
        if left.producer != ObservationProducer::Whisper {
            continue;
        }
        for (right_index, right) in slots.iter().enumerate().skip(index + 1) {
            if right.producer == ObservationProducer::Whisper
                && left.observation != right.observation
                && overlaps(left, right)
                && normalized(&left.text) != normalized(&right.text)
            {
                cross_window.push(CrossWindowOverlap {
                    reason: "whisper_cross_window_overlap",
                    slot_index: index,
                    comparison: comparison(left, right_index, right, rate),
                });
            }
        }
    }
    OccurrenceSlotReceipt {
        schema: "codescribe.occurrence_slots.v1",
        session: &occurrence.session,
        capture_epoch: occurrence.capture_epoch,
        sample_start: occurrence.sample_start,
        sample_end: occurrence.sample_end,
        sample_rate_hz: rate,
        slots: slots
            .iter()
            .map(|slot| SlotReceipt {
                producer: slot.producer.as_str(),
                sample_start: slot.sample_start,
                sample_end: slot.sample_end,
                text: &slot.text,
                observation: ObservationReceipt::from(&slot.observation),
                survived: if matches!(
                    slot.producer,
                    ObservationProducer::Apple
                        | ObservationProducer::CloudLive
                        | ObservationProducer::Lexicon
                ) {
                    windows
                        .values()
                        .filter_map(|words| classify_survival(slot, words, rate))
                        .collect()
                } else {
                    Vec::new()
                },
            })
            .collect(),
        whisper_cross_window_overlap: cross_window,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::acoustic_ledger::{AcousticEvidence, EnergyCalibration, SlotWitness};

    fn occurrence() -> OccurrenceIdentity {
        OccurrenceIdentity::new(uuid::Uuid::new_v4().to_string(), 7, 0, 48_000)
    }

    fn slot(
        owner: &OccurrenceIdentity,
        producer: ObservationProducer,
        request: u64,
        start: u64,
        end: u64,
        text: &str,
    ) -> WordSlot {
        WordSlot {
            sample_start: start,
            sample_end: end,
            text: text.into(),
            producer,
            observation: ObservationIdentity::new(producer, request, 0, owner.clone()),
            witness: SlotWitness::Unwitnessed,
        }
    }

    fn qualified(owner: &OccurrenceIdentity) -> AcousticLedger {
        let mut ledger = AcousticLedger::new();
        ledger.bind_capture_rate(48_000);
        let calibration = EnergyCalibration::new("slot-receipt-test", 1.0, 1);
        let evidence = AcousticEvidence {
            occurrence: owner.clone(),
            duration_ms: owner.sample_len() as f64 / 48.0,
            energy_integral: 100.0,
            mean_rms_dbfs: -20.0,
            peak_dbfs: -10.0,
            vad_open_sample: Some(owner.sample_start),
            vad_close_sample: Some(owner.sample_end),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        ledger.schedule_frontier(owner.clone(), [ObservationProducer::Whisper]);
        ledger
    }

    fn mixed_ledger(owner: &OccurrenceIdentity) -> AcousticLedger {
        let mut ledger = qualified(owner);
        let apple = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, owner.clone());
        ledger.admit_word_slots(&apple, &[(12_000, 21_600, "providers".into())]);
        let whisper = ObservationIdentity::new(ObservationProducer::Whisper, 2, 0, owner.clone());
        ledger.admit_word_slots(
            &whisper,
            &[
                (4_800, 14_400, "Provider".into()),
                (24_000, 33_600, "works".into()),
            ],
        );
        ledger.note_frontier_return(owner, ObservationProducer::Whisper);
        assert_eq!(ledger.slots_of(owner).unwrap().len(), 3);
        ledger
    }

    fn assert_same_ledger(
        left: &AcousticLedger,
        right: &AcousticLedger,
        owner: &OccurrenceIdentity,
    ) {
        assert_eq!(left.seal_of(owner), right.seal_of(owner));
        assert_eq!(left.slots_of(owner), right.slots_of(owner));
        assert_eq!(left.text_of(owner), right.text_of(owner));
        assert_eq!(left.layer_trail(), right.layer_trail());
        assert_eq!(left.serial_of(owner), right.serial_of(owner));
        assert_eq!(left.frontier_of(owner), right.frontier_of(owner));
        assert_eq!(left.conservation(), right.conservation());
        assert_eq!(left.compose(owner), right.compose(owner));
    }

    #[test]
    fn misaligned_apple_word_survives_in_whisper_gap_with_signed_capture_deltas() {
        let owner = occurrence();
        let ledger = mixed_ledger(&owner);
        let receipt = build_receipt(&owner, ledger.slots_of(&owner).unwrap(), 48_000);
        assert_eq!(receipt.session, owner.session);
        assert_eq!(receipt.capture_epoch, 7);
        assert_eq!(receipt.slots[1].producer, "apple");
        let evidence = &receipt.slots[1].survived[0];
        assert_eq!(
            evidence.reasons,
            ["midpoint_in_whisper_gap", "partial_overlap_midpoint_outside"]
        );
        assert_eq!(evidence.whisper_observation.request, 2);
        assert_eq!(evidence.heard_sample_start, 4_800);
        assert_eq!(evidence.heard_sample_end, 33_600);
        assert_eq!(evidence.comparison.nearest_whisper_word.text, "Provider");
        assert_eq!(evidence.comparison.nearest_whisper_word.sample_start, 4_800);
        assert_eq!(evidence.comparison.nearest_whisper_word.sample_end, 14_400);
        assert_eq!(evidence.comparison.start_delta_ms, 150.0);
        assert_eq!(evidence.comparison.end_delta_ms, 150.0);
        assert!(!evidence.comparison.same_normalized_text);
    }

    #[test]
    fn every_supported_survivor_is_classified_per_observation_without_false_outer_spans() {
        let owner = occurrence();
        for producer in [
            ObservationProducer::Apple,
            ObservationProducer::CloudLive,
            ObservationProducer::Lexicon,
        ] {
            let slots = vec![
                slot(&owner, ObservationProducer::Whisper, 1, 0, 100, "one"),
                slot(&owner, producer, 3, 120, 140, "kept"),
                slot(&owner, ObservationProducer::Whisper, 1, 200, 300, "two"),
                slot(&owner, producer, 3, 320, 340, "outside"),
                slot(&owner, ObservationProducer::Whisper, 2, 400, 500, "three"),
            ];
            let receipt = build_receipt(&owner, &slots, 1_000);
            let evidence = &receipt.slots[1].survived;
            assert_eq!(evidence.len(), 1);
            assert_eq!(
                evidence[0].reasons,
                ["midpoint_in_whisper_gap", "no_whisper_overlap"]
            );
            assert!(receipt.slots[3].survived.is_empty());
        }
    }

    #[test]
    fn midpoint_boundaries_and_nearest_word_do_not_use_spelling_as_distance() {
        let owner = occurrence();
        let slots = vec![
            slot(&owner, ObservationProducer::Apple, 3, 80, 120, "TWO!"),
            slot(&owner, ObservationProducer::Whisper, 1, 100, 150, "one"),
            slot(&owner, ObservationProducer::Whisper, 1, 200, 300, "two"),
            slot(&owner, ObservationProducer::Lexicon, 4, 210, 230, "TWO!"),
        ];
        let receipt = build_receipt(&owner, &slots, 1_000);
        let edge = &receipt.slots[0].survived[0];
        assert_eq!(edge.reasons, ["midpoint_inside_whisper_word"]);
        assert_eq!(edge.comparison.nearest_whisper_word.text, "one");
        assert_eq!(edge.comparison.start_delta_ms, -20.0);
        assert_eq!(edge.comparison.end_delta_ms, -30.0);
        assert!(!edge.comparison.same_normalized_text);
        assert!(receipt.slots[3].survived[0].comparison.same_normalized_text);
    }

    #[test]
    fn different_whisper_windows_emit_each_differently_spelled_overlap_once() {
        let owner = occurrence();
        let mut slots = vec![
            slot(&owner, ObservationProducer::Whisper, 1, 4_800, 14_400, "Provider"),
            slot(&owner, ObservationProducer::Whisper, 2, 9_600, 19_200, "providers"),
        ];
        let receipt = build_receipt(&owner, &slots, 48_000);
        let pairs = &receipt.whisper_cross_window_overlap;
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].reason, "whisper_cross_window_overlap");
        assert_eq!(pairs[0].slot_index, 0);
        assert_eq!(pairs[0].comparison.nearest_whisper_word.slot_index, 1);
        assert_eq!(pairs[0].comparison.start_delta_ms, -100.0);
        assert_eq!(pairs[0].comparison.end_delta_ms, -100.0);
        assert!(!pairs[0].comparison.same_normalized_text);
        slots[1].text = "PROVIDER!".into();
        assert!(
            build_receipt(&owner, &slots, 48_000)
                .whisper_cross_window_overlap
                .is_empty()
        );
        slots[1].text = "different".into();
        slots[1].observation = slots[0].observation.clone();
        assert!(
            build_receipt(&owner, &slots, 48_000)
                .whisper_cross_window_overlap
                .is_empty()
        );
        slots[1].observation.generation += 1;
        assert_eq!(
            build_receipt(&owner, &slots, 48_000)
                .whisper_cross_window_overlap
                .len(),
            1
        );
        slots[1].sample_start = slots[0].sample_end;
        assert!(
            build_receipt(&owner, &slots, 48_000)
                .whisper_cross_window_overlap
                .is_empty()
        );
    }

    #[test]
    fn sealing_twice_emits_one_line_and_sink_on_off_preserves_structural_truth() {
        let dir = tempfile::tempdir().unwrap();
        let owner = occurrence();
        let mut disabled = mixed_ledger(&owner);
        let mut enabled = disabled.clone();
        disabled.seal(&owner).unwrap();
        let sink = SlotReceiptSink::open_in(dir.path(), &owner.session, owner.capture_epoch, 48_000)
            .unwrap();
        let path = sidecar_path(dir.path(), &owner.session).unwrap();
        enabled.seal(&owner).unwrap();
        enabled.seal(&owner).unwrap();
        assert_same_ledger(&disabled, &enabled, &owner);
        // An automatic observation after finality and another seal never append.
        let late = ObservationIdentity::new(ObservationProducer::Whisper, 9, 9, owner.clone());
        assert_eq!(
            disabled.admit_word_slots(&late, &[(1_000, 2_000, "late".into())]),
            enabled.admit_word_slots(&late, &[(1_000, 2_000, "late".into())])
        );
        disabled.seal(&owner).unwrap();
        enabled.seal(&owner).unwrap();
        assert_same_ledger(&disabled, &enabled, &owner);
        let bytes = fs::read_to_string(&path).unwrap();
        assert_eq!(bytes.lines().count(), 1);
        let row: serde_json::Value = serde_json::from_str(bytes.trim_end()).unwrap();
        assert_eq!(row["slots"].as_array().unwrap().len(), 3);
        assert_eq!(row["session"], owner.session);
        assert_eq!(row["capture_epoch"], 7);
        assert_eq!(row["sample_rate_hz"], 48_000);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(sink);
        // A fresh ledger with the same coordinates cannot write after sink drop.
        mixed_ledger(&owner).seal(&owner).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), bytes);
    }

    #[test]
    fn refused_seal_and_unregistered_epoch_emit_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let owner = occurrence();
        let _sink =
            SlotReceiptSink::open_in(dir.path(), &owner.session, owner.capture_epoch, 48_000)
                .unwrap();
        let path = sidecar_path(dir.path(), &owner.session).unwrap();
        assert!(qualified(&owner).seal(&owner).is_err());
        let mut foreign_epoch = owner.clone();
        foreign_epoch.capture_epoch += 1;
        mixed_ledger(&foreign_epoch).seal(&foreign_epoch).unwrap();
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
    }

    #[test]
    fn unavailable_sink_and_failed_writer_cannot_change_seals() {
        let dir = tempfile::tempdir().unwrap();
        let owner = occurrence();
        let blocked_root = dir.path().join("file-not-directory");
        fs::write(&blocked_root, b"occupied").unwrap();
        assert!(SlotReceiptSink::open_in(&blocked_root, &owner.session, 7, 48_000).is_err());
        let mut disabled = mixed_ledger(&owner);
        disabled.seal(&owner).unwrap();
        let _sink = SlotReceiptSink::open_in(dir.path(), &owner.session, 7, 48_000).unwrap();
        let state = SINKS.get().unwrap().lock().unwrap()[&(owner.session.clone(), 7)].clone();
        let path = sidecar_path(dir.path(), &owner.session).unwrap();
        // A read-only descriptor in a temp directory forces write_all to fail.
        let read_only = File::open(&path).expect("temporary sidecar exists");
        *state.file.lock().unwrap() = Some(read_only);
        let mut closed = mixed_ledger(&owner);
        closed.seal(&owner).unwrap();
        assert_same_ledger(&disabled, &closed, &owner);
        assert!(state.file.lock().unwrap().is_none());
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
    }

    #[test]
    fn five_physical_iwo_occurrences_stay_five_with_five_seal_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let first = occurrence();
        let _sink = SlotReceiptSink::open_in(dir.path(), &first.session, 7, 48_000).unwrap();
        let mut ledger = AcousticLedger::new();
        let calibration = EnergyCalibration::new("five-physical-occurrences", 1.0, 1);
        for index in 0..5 {
            let owner =
                OccurrenceIdentity::new(&first.session, 7, index * 48_000, index * 48_000 + 24_000);
            let evidence = AcousticEvidence {
                occurrence: owner.clone(),
                duration_ms: 500.0,
                energy_integral: 100.0,
                mean_rms_dbfs: -20.0,
                peak_dbfs: -10.0,
                vad_open_sample: Some(owner.sample_start),
                vad_close_sample: Some(owner.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            ledger.schedule_frontier(owner.clone(), []);
            let apple =
                ObservationIdentity::new(ObservationProducer::Apple, index, 0, owner.clone());
            ledger.admit_word_slots(
                &apple,
                &[(owner.sample_start, owner.sample_end, "Iwo".into())],
            );
            ledger.seal(&owner).unwrap();
            ledger.seal(&owner).unwrap();
        }
        let coverage = OccurrenceIdentity::new(&first.session, 7, 0, 240_000);
        let composed = ledger.compose(&coverage).unwrap();
        assert_eq!(ledger.len(), 5);
        assert_eq!(composed.occurrence_count(), 5);
        assert_eq!(composed.token_count(), 5);
        let bytes = fs::read_to_string(sidecar_path(dir.path(), &first.session).unwrap()).unwrap();
        assert_eq!(bytes.lines().count(), 5);
    }

    #[test]
    #[serial_test::serial]
    fn production_resolver_places_sidecar_beside_audio_under_temp_data_dir() {
        struct RestoreDataDir(Option<std::ffi::OsString>);
        impl Drop for RestoreDataDir {
            fn drop(&mut self) {
                unsafe {
                    match self.0.take() {
                        Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                        None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                    }
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let _restore = RestoreDataDir(std::env::var_os("CODESCRIBE_DATA_DIR"));
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", dir.path()) };
        let owner = occurrence();
        let _sink = SlotReceiptSink::for_session(&owner.session, 7, 48_000).unwrap();
        mixed_ledger(&owner).seal(&owner).unwrap();
        let expected = dir
            .path()
            .join("sessions")
            .join(format!("{}.slots.jsonl", owner.session));
        assert!(expected.is_file());
        assert!(
            expected
                .canonicalize()
                .unwrap()
                .starts_with(dir.path().canonicalize().unwrap())
        );
        for invalid in ["../escape", "absolute/path", "bad:stopping", "short"] {
            assert!(sidecar_path(dir.path(), invalid).is_err());
        }
    }
}
