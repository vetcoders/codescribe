//! Acceptance tests for live formatted whole-document derived paint during active capture.

use super::*;
use crate::presentation::transcript_bus::TranscriptBusEvidenceEvent;
use codescribe_core::config::FormattingPolicy;
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink, LayerSummary};
use std::sync::{Arc, Mutex as StdMutex};

const LIVE_TAKE: &str = "formatter-live-acceptance";
const RAW_FRAGMENTS: [&str; 6] = [
    "pierwsze zdanie na zywo",
    "drugie zdanie na zywo",
    "trzecie zdanie na zywo",
    "czwarte zdanie na zywo",
    "piate zdanie na zywo",
    "szoste zdanie na zywo",
];
const FORMATTED_FRAGMENTS: [&str; 6] = [
    "Pierwsze zdanie na żywo.",
    "Drugie zdanie na żywo.",
    "Trzecie zdanie na żywo.",
    "Czwarte zdanie na żywo.",
    "Piąte zdanie na żywo.",
    "Szóste zdanie na żywo.",
];

struct LiveFixture {
    emitter: Arc<PresentationEmitter>,
    sources: Vec<ObservationIdentity>,
    derived_events: Arc<StdMutex<Vec<TranscriptBusEvidenceEvent>>>,
    _dir: tempfile::TempDir,
}

impl LiveFixture {
    fn open_and_admit_six() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(
            TranscriptBus::open_at(
                crate::presentation::transcript_bus::TranscriptSession {
                    session_id: LIVE_TAKE.into(),
                    mode: crate::presentation::transcript_bus::TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                dir.path().join("live-bus.jsonl"),
                None,
            )
            .expect("bus"),
        );
        bus.publish_started();
        let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
        let derived_events: Arc<StdMutex<Vec<TranscriptBusEvidenceEvent>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let derived_sink = Arc::clone(&derived_events);
        let projection_callback: ProjectionObserver =
            Arc::new(move |event: &TranscriptBusEvidenceEvent| {
                if event.reducer_action == "derived_projection" {
                    derived_sink.lock().expect("lock").push(event.clone());
                }
            });
        let emitter = Arc::new(PresentationEmitter::new_with_authority(
            Arc::new(tokio::sync::Mutex::new(String::new())),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            Some(projection_callback),
        ));
        emitter.on_capture_opened(LIVE_TAKE, 1);

        let mut sources = Vec::with_capacity(RAW_FRAGMENTS.len());
        for (index, label) in RAW_FRAGMENTS.iter().enumerate() {
            let start = index as u64 * 16_000;
            let end = start + 16_000;
            let occurrence = OccurrenceIdentity::new(LIVE_TAKE, 1, start, end);
            let source = ObservationIdentity::new(
                ObservationProducer::Apple,
                index as u64 + 1,
                0,
                occurrence.clone(),
            );
            let (mutation, seal) = {
                let mut guard = ledger.lock().expect("ledger");
                let calibration = EnergyCalibration {
                    version: "formatter-live-acceptance".into(),
                    min_energy_integral: 1.0,
                    min_valley_samples: 1,
                };
                let evidence = AcousticEvidence {
                    occurrence: occurrence.clone(),
                    duration_ms: 1000.0,
                    energy_integral: 10.0,
                    mean_rms_dbfs: -12.0,
                    peak_dbfs: -3.0,
                    vad_open_sample: Some(start),
                    vad_close_sample: Some(end),
                    evidence_calibration_version: calibration.version.clone(),
                };
                assert!(guard.qualify(&evidence, &calibration).is_qualified());
                let receipt = guard.admit(&source, label);
                let mutation = EngineEvent::LedgerMutation {
                    observation: source.clone(),
                    label: (*label).into(),
                    receipt,
                };
                guard.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(guard.note_frontier_return(&occurrence, ObservationProducer::Apple));
                (mutation, guard.seal(&occurrence).expect("seal").clone())
            };
            emitter.on_event(&mutation);
            emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
            sources.push(source);
        }

        Self {
            emitter,
            sources,
            derived_events,
            _dir: dir,
        }
    }

    fn propose(
        &self,
        index: usize,
        text: &str,
        policy: FormattingPolicy,
        disposition: LabelProposalDisposition,
    ) {
        let source = self.sources[index].clone();
        let occurrence = &source.occurrence;
        self.emitter
            .on_event(&EngineEvent::OccurrenceLabelProposal {
                proposal: OccurrenceLabelProposal::for_existing_occurrence(
                    occurrence.session.clone(),
                    occurrence.capture_epoch,
                    occurrence.sample_start,
                    occurrence.sample_end,
                    text,
                    disposition,
                )
                .with_source(source, RAW_FRAGMENTS[index].into(), policy),
            });
    }
}

#[tokio::test]
async fn live_formatter_paints_formatted_prefix_with_raw_suffix_before_stop_and_falls_back_on_partial_failure()
 {
    let fixture = LiveFixture::open_and_admit_six();
    let raw_full = RAW_FRAGMENTS.join(" ");
    let canvas_floor = fixture
        .emitter
        .visible_canvas_snapshot()
        .expect("canvas")
        .text;
    assert_eq!(
        canvas_floor,
        "Pierwsze zdanie na zywo drugie zdanie na zywo trzecie zdanie na zywo czwarte zdanie na zywo piate zdanie na zywo szoste zdanie na zywo"
    );

    // Format the first 4 fragments while the take is still capturing (before SessionFinalised).
    for (index, formatted) in FORMATTED_FRAGMENTS.iter().enumerate().take(4) {
        fixture.propose(
            index,
            formatted,
            FormattingPolicy::Smart,
            LabelProposalDisposition::Propose,
        );
    }

    let events = fixture.derived_events.lock().expect("events").clone();
    let latest_live = events
        .iter()
        .rev()
        .find(|event| !event.terminal)
        .expect("non-terminal derived_projection published during active capture");
    let expected_live = format!(
        "{} {} {} {} {} {}",
        FORMATTED_FRAGMENTS[0],
        FORMATTED_FRAGMENTS[1],
        FORMATTED_FRAGMENTS[2],
        FORMATTED_FRAGMENTS[3],
        RAW_FRAGMENTS[4],
        RAW_FRAGMENTS[5]
    );
    assert_eq!(
        latest_live.delivery_text.as_deref(),
        Some(expected_live.as_str())
    );
    assert_eq!(latest_live.rendered_text, canvas_floor);

    // Stale source label is refused and cannot overwrite the live view.
    let stale_source = fixture.sources[0].clone();
    fixture
        .emitter
        .on_event(&EngineEvent::OccurrenceLabelProposal {
            proposal: OccurrenceLabelProposal::for_existing_occurrence(
                LIVE_TAKE,
                1,
                stale_source.occurrence.sample_start,
                stale_source.occurrence.sample_end,
                "Stale replacement.",
                LabelProposalDisposition::Propose,
            )
            .with_source(
                stale_source,
                "nieaktualny tekst".into(),
                FormattingPolicy::Smart,
            ),
        });
    let after_stale = fixture
        .derived_events
        .lock()
        .expect("events")
        .iter()
        .rev()
        .find(|event| !event.terminal)
        .expect("live_view")
        .delivery_text
        .clone();
    assert_eq!(after_stale.as_deref(), Some(expected_live.as_str()));

    // Fragment 5 fails and the take ends: Stop selection falls back to complete Raw floor, never partial 1..4.
    fixture.propose(
        4,
        "",
        FormattingPolicy::Smart,
        LabelProposalDisposition::Refuse,
    );
    fixture.emitter.on_event(&EngineEvent::SessionFinalised {
        session_id: LIVE_TAKE.into(),
        layer_summary: LayerSummary::default(),
    });
    let terminal_selection = fixture
        .emitter
        .delivery_projection(FormattingPolicy::Smart, &raw_full)
        .expect("terminal delivery projection");
    assert_eq!(terminal_selection.status, "unavailable");
    assert_eq!(terminal_selection.rendered_text, raw_full);
    assert_eq!(
        fixture
            .emitter
            .visible_canvas_snapshot()
            .expect("canvas")
            .text,
        format!("{canvas_floor}.")
    );
}
