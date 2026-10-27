//! Independent acceptance of the production selector and its published handoff.
//! These fixtures admit synthetic measured occurrences; they do not open a mic
//! or substitute for installed capture/provider acceptance.
use super::*;
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::{EventSink, LayerSummary};

const TAKE: &str = "formatter-selection-acceptance";
const LABEL: &str = "niepoprawne zdanie";

struct Fixture {
    controller: RecordingController,
    emitter: Arc<PresentationEmitter>,
    source: ObservationIdentity,
    raw: String,
    revision: u64,
    path: std::path::PathBuf,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let controller = RecordingController::new_without_keychain();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bus.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                crate::presentation::transcript_bus::TranscriptSession {
                    session_id: TAKE.into(),
                    mode: crate::presentation::transcript_bus::TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                path.clone(),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let emitter = Arc::new(PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            None,
        ));
        emitter.on_capture_opened(TAKE, 1);
        let occurrence = OccurrenceIdentity::new(TAKE, 1, 0, 16_000);
        let source = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence.clone());
        let (mutation, seal) = {
            let mut ledger = ledger.lock().unwrap();
            let calibration = EnergyCalibration {
                version: "formatter-acceptance-synthetic".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            };
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(0),
                vad_close_sample: Some(16_000),
                evidence_calibration_version: calibration.version.clone(),
            };
            assert!(ledger.qualify(&evidence, &calibration).is_qualified());
            let receipt = ledger.admit(&source, LABEL);
            let mutation = EngineEvent::LedgerMutation {
                observation: source.clone(),
                label: LABEL.into(),
                receipt,
            };
            ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
            assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
            (mutation, ledger.seal(&occurrence).unwrap().clone())
        };
        emitter.on_event(&mutation);
        emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: TAKE.into(),
            layer_summary: LayerSummary::default(),
        });
        let request = emitter.terminal_formatter_request().unwrap();
        *controller.active_presentation.write().await = Some(Arc::clone(&emitter));
        Self {
            controller,
            emitter,
            source,
            raw: request.source_text,
            revision: request.source_revision,
            path,
            _directory: directory,
        }
    }

    fn propose(emitter: &PresentationEmitter, source: ObservationIdentity, text: &str) {
        let occurrence = &source.occurrence;
        emitter.on_event(&EngineEvent::OccurrenceLabelProposal {
            proposal: OccurrenceLabelProposal::for_existing_occurrence(
                occurrence.session.clone(),
                occurrence.capture_epoch,
                occurrence.sample_start,
                occurrence.sample_end,
                text,
                LabelProposalDisposition::Propose,
            )
            .with_source(source, LABEL.into(), FormattingPolicy::Smart),
        });
    }
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_waits_for_delayed_result_and_freezes_receipt_payload() {
    let fixture = Fixture::new().await;
    let settings = fixture.controller.runtime_settings_arc().await;
    assert_eq!(settings.formatting_policy(), FormattingPolicy::Smart);
    fixture.emitter.on_formatter_pending(true);
    let emitter = Arc::clone(&fixture.emitter);
    let source = fixture.source.clone();
    let job = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        Fixture::propose(&emitter, source, "Poprawione zdanie.");
        emitter.on_formatter_pending(false);
    });
    let selected = fixture
        .controller
        .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
        .await;
    job.await.unwrap();
    assert_eq!(selected, "Poprawione zdanie.");
    fixture
        .emitter
        .record_projection_delivery(&selected, TranscriptDelivery::CopiedToClipboard);
    Fixture::propose(
        &fixture.emitter,
        fixture.source.clone(),
        "Late replacement.",
    );
    let frozen = fixture
        .emitter
        .delivery_projection(FormattingPolicy::Smart, &fixture.raw)
        .unwrap();
    assert_eq!(frozen.rendered_text, selected);
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(&fixture.path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|row| row["schema"] == "codescribe.projection-delivery.v1")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["source_raw_revision"], fixture.revision);
    assert_eq!(rows[0]["selected_mode"], "smart");
    assert_eq!(rows[0]["payload_utf8"], selected);
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_timeout_keeps_one_frozen_fallback() {
    let fixture = Fixture::new().await;
    let settings = fixture.controller.runtime_settings_arc().await;
    assert_eq!(settings.formatting_policy(), FormattingPolicy::Smart);
    fixture.emitter.on_formatter_pending(true);
    let selected = fixture
        .controller
        .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
        .await;
    assert_eq!(selected, fixture.raw);
    Fixture::propose(&fixture.emitter, fixture.source.clone(), "Too late.");
    fixture.emitter.on_formatter_pending(false);
    let repeated = fixture
        .controller
        .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
        .await;
    assert_eq!(repeated, selected);
}

#[tokio::test]
async fn formatter_selection_cannot_borrow_another_take_or_override_literal() {
    let fixture = Fixture::new().await;
    let settings = fixture.controller.runtime_settings_arc().await;
    Fixture::propose(
        &fixture.emitter,
        fixture.source.clone(),
        "Poprawione zdanie.",
    );
    let foreign = fixture
        .controller
        .select_paste_projection(Some("another-take"), &settings, "other raw".into())
        .await;
    assert_eq!(foreign, "other raw");
    fixture.emitter.set_literal_delivery(true);
    let literal = fixture
        .controller
        .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
        .await;
    assert_eq!(literal, fixture.raw);
}
