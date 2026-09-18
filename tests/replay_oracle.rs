use std::collections::BTreeMap;

use chrono::DateTime;
use codescribe::presentation::transcript_projection::{
    TranscriptProjection, TranscriptProjectionKind, TranscriptProjectionReader,
};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/session_6903184c_bus.jsonl");
const SESSION_ID: &str = "6903184c-1f09-4aad-aae7-836108924970";

// Pinned from the independent offline decode of the same WAV (intake,
// Dopisek IV: 1,912 characters / 36 segments / avg_logprob -0.41), not
// calculated from the projection path exercised below.
const EXPECTED_REVISION: u64 = 20;
const EXPECTED_SENTENCE_1: &str = "Mieliśmy plany nate zintegrowane.";
const EXPECTED_SENTENCE_2: &str = "Niesamowitą diagnostykę.";
const EXPECTED_SENTENCE_3: &str = "Nie mając w ogóle transkrypcji de facto i czego.";
const EXPECTED_TEXT: &str = "Mieliśmy plany nate zintegrowane. Niesamowitą diagnostykę. Nie mając w ogóle transkrypcji de facto i czego.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanvasMode {
    Listening,
    Formatted,
    NoSpeech,
}

#[derive(Debug, PartialEq, Eq)]
struct CanvasState {
    mode: CanvasMode,
    committed_text: String,
    revision: Option<u64>,
    terminal_projection_seen: bool,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            mode: CanvasMode::Listening,
            committed_text: String::new(),
            revision: None,
            terminal_projection_seen: false,
        }
    }
}

impl CanvasState {
    fn apply_projection(&mut self, projection: TranscriptProjection) {
        self.committed_text = projection.rendered_text;
        self.revision = Some(projection.reducer_revision);
        if projection.kind == TranscriptProjectionKind::TerminalSeal {
            self.terminal_projection_seen = true;
            self.mode = if self.committed_text.trim().is_empty() {
                CanvasMode::NoSpeech
            } else {
                CanvasMode::Formatted
            };
        }
    }

    fn apply_session_ended(&mut self) {
        if !self.terminal_projection_seen {
            self.mode = CanvasMode::NoSpeech;
        }
    }
}

fn fixture_rows() -> Vec<Value> {
    FIXTURE
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("fixture row {} is invalid: {error}", index + 1))
        })
        .collect()
}

fn replay_projection_effect() -> CanvasState {
    let mut reader = TranscriptProjectionReader::new();
    let mut canvas = CanvasState::default();

    for line in FIXTURE.lines() {
        let row: Value = serde_json::from_str(line).expect("fixture row must be valid JSON");
        let session_ended = row.get("status").and_then(Value::as_str) == Some("session_ended");

        if let Some(projection) = reader
            .push_line(line)
            .expect("production projection reader must accept the captured Bus row")
        {
            canvas.apply_projection(projection);
        }
        if session_ended {
            canvas.apply_session_ended();
        }
    }

    canvas
}

#[test]
fn fixture_preserves_the_34_row_chronology_and_terminal_gap() {
    let rows = fixture_rows();
    assert_eq!(rows.len(), 34);

    let sequences = rows
        .iter()
        .map(|row| row["sequence"].as_u64().expect("numeric sequence"))
        .collect::<Vec<_>>();
    assert_eq!(sequences, (1..=34).collect::<Vec<_>>());
    assert!(rows.iter().all(|row| row["session_id"] == SESSION_ID));

    let mut types = BTreeMap::<&str, usize>::new();
    for row in &rows {
        let event_type = row
            .get("reducer_action")
            .or_else(|| row.get("status"))
            .and_then(Value::as_str)
            .expect("every row names its event type");
        *types.entry(event_type).or_default() += 1;
    }
    assert_eq!(types.get("session_started"), Some(&1));
    assert_eq!(types.get("apply_ledger_decision"), Some(&13));
    assert_eq!(types.get("record_ledger_seal"), Some(&19));
    assert_eq!(types.get("session_ended"), Some(&1));
    assert_eq!(types.get("record_seal_coverage"), None);
    assert_eq!(types.get("record_ledger_terminal_seal"), None);

    let timestamps = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            DateTime::parse_from_rfc3339(
                row["emitted_at"]
                    .as_str()
                    .unwrap_or_else(|| panic!("fixture row {} needs a timestamp", index + 1)),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "fixture row {} has an invalid timestamp: {error}",
                    index + 1
                )
            })
        })
        .collect::<Vec<_>>();
    assert!(
        timestamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "captured Bus chronology must never move backwards"
    );

    let terminal_gap = timestamps[33].signed_duration_since(timestamps[32]);
    assert_eq!(terminal_gap.num_milliseconds(), 92_162);
    assert_eq!((terminal_gap.num_milliseconds() + 999) / 1_000, 93);
}

#[test]
fn session_6903184c_ends_formatted_from_committed_revision_20() {
    let canvas = replay_projection_effect();

    assert_eq!(canvas.revision, Some(EXPECTED_REVISION));
    assert_eq!(canvas.committed_text, EXPECTED_TEXT);
    assert!(canvas.committed_text.contains(EXPECTED_SENTENCE_1));
    assert!(canvas.committed_text.contains(EXPECTED_SENTENCE_2));
    assert!(canvas.committed_text.contains(EXPECTED_SENTENCE_3));
    assert_eq!(
        canvas.mode,
        CanvasMode::Formatted,
        "session_ended must derive terminal canvas mode from the last committed projection"
    );
}

#[test]
fn empty_committed_book_ends_no_speech() {
    let mut reader = TranscriptProjectionReader::new();
    let started = serde_json::json!({
        "schema": "codescribe.transcript.v1",
        "sequence": 1,
        "session_id": "empty-session",
        "status": "session_started"
    });
    let ended = serde_json::json!({
        "schema": "codescribe.transcript.v1",
        "sequence": 2,
        "session_id": "empty-session",
        "status": "session_ended"
    });
    assert!(reader.push_line(&started.to_string()).unwrap().is_none());
    let terminal = reader
        .push_line(&ended.to_string())
        .unwrap()
        .expect("session_ended must project an empty committed book");

    let mut canvas = CanvasState::default();
    canvas.apply_projection(terminal);
    assert_eq!(canvas.mode, CanvasMode::NoSpeech);
    assert_eq!(canvas.revision, Some(0));
    assert!(canvas.committed_text.is_empty());
    assert!(canvas.terminal_projection_seen);
}
