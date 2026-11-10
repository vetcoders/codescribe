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
        Self::open_for_document(LIVE_TAKE)
    }

    fn open_for_document(document_id: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(
            TranscriptBus::open_at(
                crate::presentation::transcript_bus::TranscriptSession {
                    session_id: document_id.into(),
                    mode: crate::presentation::transcript_bus::TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                dir.path().join("live-bus.jsonl"),
                None,
            )
            .expect("bus")
            .with_capture_session(LIVE_TAKE.into()),
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
async fn live_formatter_keeps_accepted_prefix_at_stop_when_another_fragment_fails() {
    let fixture = LiveFixture::open_and_admit_six();
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

    // One failed fragment cannot discard the four accepted live fragments.
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
    let raw_full = fixture
        .emitter
        .terminal_formatter_request()
        .expect("owned terminal document")
        .source_text;
    let terminal_selection = fixture
        .emitter
        .delivery_projection(FormattingPolicy::Smart, &raw_full)
        .expect("terminal delivery projection");
    assert_eq!(terminal_selection.status, "applied");
    assert_eq!(
        terminal_selection.rendered_text,
        format!("{expected_live}.")
    );
    assert_eq!(
        fixture
            .emitter
            .visible_canvas_snapshot()
            .expect("canvas")
            .text,
        format!("{canvas_floor}.")
    );
}

#[tokio::test]
async fn stop_preserves_current_live_correction_for_distinct_document_and_capture() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        let fixture = LiveFixture::open_for_document("owned-dictation-document");
        fixture.propose(
            0,
            FORMATTED_FRAGMENTS[0],
            policy,
            LabelProposalDisposition::Propose,
        );
        fixture.propose(1, "", policy, LabelProposalDisposition::Refuse);
        fixture.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: LIVE_TAKE.into(),
            layer_summary: LayerSummary::default(),
        });
        let raw = fixture
            .emitter
            .terminal_formatter_request()
            .expect("owned terminal document")
            .source_text;
        assert!(
            fixture
                .emitter
                .delivery_projection_with_assembly(
                    Some("foreign-document"),
                    policy,
                    &raw,
                    ProjectionAssembly::TerminalDocument
                )
                .is_none(),
            "a different document cannot select this capture's formatting"
        );
        let selected = fixture
            .emitter
            .delivery_projection_with_assembly(
                Some("owned-dictation-document"),
                policy,
                &raw,
                ProjectionAssembly::TerminalDocument,
            )
            .expect("source-bound delivery for the owning document");
        assert_eq!(selected.status, "applied");
        assert_eq!(selected.delivered_mode, policy.as_str());
        assert_eq!(
            selected.rendered_text,
            format!(
                "{} {}.",
                FORMATTED_FRAGMENTS[0],
                RAW_FRAGMENTS[1..].join(" ")
            )
        );
        // Late formatting is history, not authority for a second handoff.
        fixture.propose(
            2,
            FORMATTED_FRAGMENTS[2],
            policy,
            LabelProposalDisposition::Propose,
        );
        assert_eq!(
            fixture
                .emitter
                .delivery_projection_with_assembly(
                    Some("owned-dictation-document"),
                    policy,
                    &raw,
                    ProjectionAssembly::TerminalDocument
                )
                .unwrap(),
            selected
        );
    }
}

#[tokio::test]
async fn live_formatter_never_oscillates_to_raw_across_apple_tail_commit_and_seal() {
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
            dir.path().join("live-no-flicker.jsonl"),
            None,
        )
        .expect("bus")
        .with_capture_session(LIVE_TAKE.into()),
    );
    bus.publish_started();
    let ledger = Arc::new(StdMutex::new(AcousticLedger::new()));
    let all_events: Arc<StdMutex<Vec<TranscriptBusEvidenceEvent>>> =
        Arc::new(StdMutex::new(Vec::new()));
    let sink = Arc::clone(&all_events);
    let projection_callback: ProjectionObserver =
        Arc::new(move |event: &TranscriptBusEvidenceEvent| {
            sink.lock().expect("lock").push(event.clone());
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

    let calibration = EnergyCalibration {
        version: "formatter-live-acceptance".into(),
        min_energy_integral: 1.0,
        min_valley_samples: 1,
    };
    let admit_and_seal = |index: u64, start: u64, end: u64, label: &str| {
        let occurrence = OccurrenceIdentity::new(LIVE_TAKE, 1, start, end);
        let source =
            ObservationIdentity::new(ObservationProducer::Apple, index, 0, occurrence.clone());
        let (mutation, seal) = {
            let mut guard = ledger.lock().expect("ledger");
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
                label: label.into(),
                receipt,
            };
            guard.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(guard.note_frontier_return(&occurrence, ObservationProducer::Apple));
            (mutation, guard.seal(&occurrence).expect("seal").clone())
        };
        (source, mutation, EngineEvent::LedgerSeal { receipt: seal })
    };

    let (source_0, mutation_0, seal_0) = admit_and_seal(1, 0, 16_000, RAW_FRAGMENTS[0]);
    emitter.on_event(&mutation_0);
    emitter.on_event(&seal_0);

    let smart_first = "<unclear>Pierwsze zdanie na żywo.</unclear>";
    emitter.on_event(&EngineEvent::OccurrenceLabelProposal {
        proposal: OccurrenceLabelProposal::for_existing_occurrence(
            LIVE_TAKE,
            1,
            0,
            16_000,
            smart_first,
            LabelProposalDisposition::Propose,
        )
        .with_source(source_0, RAW_FRAGMENTS[0].into(), FormattingPolicy::Smart),
    });

    all_events.lock().expect("lock").clear();

    // Provisional Apple words arrive for the second phrase, then the exact same
    // phrase is committed and sealed by the ledger.
    emitter.on_event(&EngineEvent::UnadmittedAppleWords {
        revision: 1,
        words: vec![
            codescribe_core::pipeline::contracts::UnadmittedAppleWord {
                sample_start: 16_000,
                sample_end: 24_000,
                text: "drugie".into(),
                source: codescribe_core::pipeline::contracts::UnadmittedAppleWordSource::Pending {
                    utterance_id: 1,
                },
            },
            codescribe_core::pipeline::contracts::UnadmittedAppleWord {
                sample_start: 24_000,
                sample_end: 32_000,
                text: "zdanie".into(),
                source: codescribe_core::pipeline::contracts::UnadmittedAppleWordSource::Pending {
                    utterance_id: 1,
                },
            },
        ],
        closed_phrases: std::collections::BTreeMap::new(),
    });

    let (_, mutation_1, seal_1) = admit_and_seal(2, 16_000, 32_000, "drugie zdanie");
    emitter.on_event(&mutation_1);
    emitter.on_event(&seal_1);

    let observed = all_events.lock().expect("lock").clone();
    assert!(
        !observed.is_empty(),
        "live view must republish across provisional tail, commit and seal"
    );
    for event in &observed {
        assert_eq!(
            event.reducer_action, "derived_projection",
            "active capture with a valid live formatted fragment must never flash a raw projection: {event:?}"
        );
        assert_eq!(
            event.delivery_text.as_deref(),
            Some("<unclear>Pierwsze zdanie na żywo.</unclear> drugie zdanie")
        );
    }
    let last = observed.last().expect("last event");
    assert_eq!(last.rendered_text, "Pierwsze zdanie na zywo drugie zdanie");
}
