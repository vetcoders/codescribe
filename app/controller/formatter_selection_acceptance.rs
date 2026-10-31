//! Independent acceptance of the production selector and its published handoff.
//! These fixtures admit synthetic measured occurrences; they do not open a mic
//! or substitute for installed capture/provider acceptance.
use super::*;
use codescribe_core::config::CapturedRuntimeInputs;
use codescribe_core::llm::inline_format::{LabelProposalDisposition, OccurrenceLabelProposal};
use codescribe_core::pipeline::acoustic_ledger::{
    AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
    OccurrenceIdentity,
};
use codescribe_core::pipeline::contracts::{EventSink, LayerSummary};

const TAKE: &str = "formatter-selection-acceptance";
const LABEL: &str = "niepoprawne zdanie";
const FRAGMENTS: [&str; 6] = [
    "pierwszy fragment nagrania",
    "drugi fragment nagrania",
    "trzeci fragment nagrania",
    "czwarty fragment nagrania",
    "piaty fragment nagrania",
    "szosty fragment nagrania",
];
const FORMATTED_FRAGMENTS: [&str; 6] = [
    "Pierwszy wynik.",
    "Drugi wynik.",
    "Trzeci wynik.",
    "Czwarty wynik.",
    "Piaty wynik.",
    "Szosty wynik.",
];

struct Fixture {
    controller: RecordingController,
    recorder: StreamingRecorder,
    emitter: Arc<PresentationEmitter>,
    sources: Vec<ObservationIdentity>,
    labels: Vec<String>,
    raw: String,
    revision: u64,
    path: std::path::PathBuf,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_policy(FormattingPolicy::Smart, true).await
    }

    async fn with_policy(policy: FormattingPolicy, terminal: bool) -> Self {
        Self::with_labels(policy, terminal, &[LABEL]).await
    }

    async fn with_labels(policy: FormattingPolicy, terminal: bool, labels: &[&str]) -> Self {
        assert!(!labels.is_empty());
        let controller = RecordingController::new_without_keychain();
        let directory = tempfile::tempdir().unwrap();
        let mut inputs = CapturedRuntimeInputs::defaults_at(directory.path().to_path_buf(), 1);
        inputs.user_settings.formatting_level = Some(policy.as_str().into());
        inputs.user_settings.seal_ledger_enabled = Some(true);
        *controller.runtime_settings.write().await =
            Arc::new(Config::runtime_snapshot_from_captured(inputs));
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
        let mut recorder = StreamingRecorder::new().unwrap();
        let ledger =
            recorder.bind_session_authority(TAKE.into(), controller.runtime_settings_arc().await);
        let emitter = Arc::new(PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            None,
        ));
        emitter.on_capture_opened(TAKE, 1);
        let mut sources = Vec::with_capacity(labels.len());
        for (index, label) in labels.iter().enumerate() {
            let start = index as u64 * 16_000;
            let end = start + 16_000;
            let occurrence = OccurrenceIdentity::new(TAKE, 1, start, end);
            let source = ObservationIdentity::new(
                ObservationProducer::Apple,
                index as u64 + 1,
                0,
                occurrence.clone(),
            );
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
                    vad_open_sample: Some(start),
                    vad_close_sample: Some(end),
                    evidence_calibration_version: calibration.version.clone(),
                };
                assert!(ledger.qualify(&evidence, &calibration).is_qualified());
                let receipt = ledger.admit(&source, label);
                let mutation = EngineEvent::LedgerMutation {
                    observation: source.clone(),
                    label: (*label).into(),
                    receipt,
                };
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                assert!(ledger.note_frontier_return(&occurrence, ObservationProducer::Apple));
                (mutation, ledger.seal(&occurrence).unwrap().clone())
            };
            emitter.on_event(&mutation);
            emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
            sources.push(source);
        }
        if terminal {
            emitter.on_event(&EngineEvent::SessionFinalised {
                session_id: TAKE.into(),
                layer_summary: LayerSummary::default(),
            });
        }
        let canvas = emitter.visible_canvas_snapshot().unwrap();
        *controller.active_presentation.write().await = Some(Arc::clone(&emitter));
        Self {
            controller,
            recorder,
            emitter,
            sources,
            labels: labels.iter().map(|label| (*label).into()).collect(),
            raw: canvas.text,
            revision: canvas.revision,
            path,
            _directory: directory,
        }
    }

    fn propose(emitter: &PresentationEmitter, source: ObservationIdentity, text: &str) {
        Self::propose_for_policy(emitter, source, text, FormattingPolicy::Smart);
    }

    fn propose_for_policy(
        emitter: &PresentationEmitter,
        source: ObservationIdentity,
        text: &str,
        policy: FormattingPolicy,
    ) {
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
            .with_source(source, LABEL.into(), policy),
        });
    }

    fn complete_fragment(
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
                .with_source(source, self.labels[index].clone(), policy),
            });
    }

    async fn deliver_once(&self, selected: &str) -> Option<serde_json::Value> {
        let settings = self.controller.runtime_settings_arc().await;
        let config = Config {
            paste_mode: PasteMode::Comfort,
            quick_notes_enabled: false,
            transcript_tagging_enabled: false,
            ..settings.values().clone()
        };
        let payloads = std::sync::Mutex::new(Vec::new());
        let delivered = self
            .controller
            .deliver_stop_transcript_with_sink(
                Some(TAKE),
                selected,
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |_, payload, _| {
                    payloads.lock().unwrap().push(payload);
                    std::future::ready(Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::PasteRequested,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    }))
                },
            )
            .await
            .unwrap();
        assert_eq!(delivered, TranscriptDelivery::SinkAccepted);
        assert_eq!(payloads.lock().unwrap().as_slice(), &[selected.to_string()]);
        assert!(
            self.controller
                .deliver_stop_transcript_with_sink(
                    Some(TAKE),
                    selected,
                    (false, false, CaptureTurnIntent::HandsFree, false),
                    &config,
                    |_, _, _| async { panic!("a repeated Stop reached the sink twice") },
                )
                .await
                .is_err()
        );
        let mut receipts: Vec<serde_json::Value> = std::fs::read_to_string(&self.path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|row: &serde_json::Value| row["schema"] == "codescribe.projection-delivery.v1")
            .collect();
        let receipt = if settings.formatting_policy() == FormattingPolicy::Off {
            assert!(
                receipts.is_empty(),
                "Raw delivery needs no formatter version"
            );
            None
        } else {
            assert_eq!(receipts.len(), 1);
            let receipt = receipts.pop().unwrap();
            assert_eq!(receipt["source_raw_revision"], self.revision);
            assert_eq!(receipt["payload_utf8"], selected);
            Some(receipt)
        };
        assert!(self.controller.conversation_engine.lock().await.is_none());
        assert!(self.controller.max_consultation.lock().await.is_none());
        assert!(self.controller.agent_channels.lock().await.is_empty());
        receipt
    }
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_absent_provider_delivers_full_raw_without_waiting() {
    for policy in [
        FormattingPolicy::Off,
        FormattingPolicy::Correction,
        FormattingPolicy::Smart,
        FormattingPolicy::Max,
    ] {
        let fixture = Fixture::with_labels(policy, true, &FRAGMENTS).await;
        let settings = fixture.controller.runtime_settings_arc().await;
        let started = tokio::time::Instant::now();
        let selected = fixture
            .controller
            .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
            .await;
        assert_eq!(selected, fixture.raw, "{policy:?}: absent AI lost Raw");
        assert_eq!(tokio::time::Instant::now(), started);
        for fragment in FRAGMENTS {
            assert!(selected.to_lowercase().contains(fragment));
        }
        let receipt = fixture.deliver_once(&selected).await;
        if policy != FormattingPolicy::Off {
            assert_eq!(receipt.unwrap()["selected_mode"], "correction");
        }
    }
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_failure_after_four_fragments_delivers_full_raw_once() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        for (failure, refused_reply, pending) in [
            ("provider_error", true, true),
            ("owner_closed", false, false),
            ("timeout", false, true),
        ] {
            let fixture = Fixture::with_labels(policy, true, &FRAGMENTS).await;
            let settings = fixture.controller.runtime_settings_arc().await;
            for (index, formatted) in FORMATTED_FRAGMENTS.iter().enumerate().take(4) {
                fixture.complete_fragment(
                    index,
                    formatted,
                    policy,
                    LabelProposalDisposition::Propose,
                );
            }
            if refused_reply {
                fixture.complete_fragment(4, "", policy, LabelProposalDisposition::Refuse);
            }
            fixture.emitter.on_formatter_pending(pending);
            let started = tokio::time::Instant::now();
            let selected = fixture
                .controller
                .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
                .await;
            assert_eq!(
                selected, fixture.raw,
                "{policy:?}/{failure}: partial AI replaced Raw"
            );
            if failure == "timeout" {
                assert_eq!(
                    tokio::time::Instant::now() - started,
                    settings.ai_execution().request_timing().attempt_timeout(),
                    "the unavailable provider cannot hold delivery beyond its budget"
                );
            } else {
                assert_eq!(tokio::time::Instant::now(), started);
            }
            let receipt = fixture.deliver_once(&selected).await.unwrap();
            assert_eq!(receipt["selected_mode"], "correction");
            let frozen = fixture
                .emitter
                .delivery_projection(policy, &fixture.raw)
                .unwrap();
            assert_eq!(frozen.status, "unavailable");
            assert_eq!(frozen.source_raw_text, fixture.raw);
            assert_eq!(
                fixture.emitter.visible_canvas_snapshot().unwrap().text,
                fixture.raw,
                "formatter results must not overwrite the independent Raw document"
            );
            // The provider comes back after Raw was handed off. Completing
            // the suffix must not change that take's chosen version or paste.
            for (index, formatted) in FORMATTED_FRAGMENTS.iter().enumerate().skip(4) {
                fixture.complete_fragment(
                    index,
                    formatted,
                    policy,
                    LabelProposalDisposition::Propose,
                );
            }
            fixture.emitter.on_formatter_pending(false);
            let repeated = fixture
                .controller
                .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
                .await;
            assert_eq!(repeated, selected);
            assert_eq!(
                fixture
                    .emitter
                    .delivery_projection(policy, &fixture.raw)
                    .unwrap(),
                frozen,
                "late completion changed the frozen receipt"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_all_fragments_formatted_is_a_positive_control() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        let fixture = Fixture::with_labels(policy, true, &FRAGMENTS).await;
        let settings = fixture.controller.runtime_settings_arc().await;
        for (index, formatted) in FORMATTED_FRAGMENTS.iter().enumerate() {
            fixture.complete_fragment(index, formatted, policy, LabelProposalDisposition::Propose);
        }
        let selected = fixture
            .controller
            .select_paste_projection(Some(TAKE), &settings, fixture.raw.clone())
            .await;
        assert_eq!(selected, FORMATTED_FRAGMENTS.join(" "));
        assert_ne!(
            selected, fixture.raw,
            "the failure fixtures must exercise usable AI results"
        );
        let receipt = fixture.deliver_once(&selected).await.unwrap();
        assert_eq!(receipt["selected_mode"], policy.as_str());
        assert_eq!(
            fixture.emitter.visible_canvas_snapshot().unwrap().text,
            fixture.raw
        );
    }
}

#[tokio::test(start_paused = true)]
async fn formatter_selection_waits_for_delayed_result_and_freezes_receipt_payload() {
    let fixture = Fixture::new().await;
    let settings = fixture.controller.runtime_settings_arc().await;
    assert_eq!(settings.formatting_policy(), FormattingPolicy::Smart);
    fixture.emitter.on_formatter_pending(true);
    let emitter = Arc::clone(&fixture.emitter);
    let source = fixture.sources[0].clone();
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
        fixture.sources[0].clone(),
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
    Fixture::propose(&fixture.emitter, fixture.sources[0].clone(), "Too late.");
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
        fixture.sources[0].clone(),
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

#[tokio::test(start_paused = true)]
async fn formatter_selection_first_stop_defers_pending_nonterminal_take() {
    for policy in [FormattingPolicy::Correction, FormattingPolicy::Smart] {
        let mut fixture = Fixture::with_policy(policy, false).await;
        let settings = fixture.controller.runtime_settings_arc().await;
        assert_eq!(settings.formatting_policy(), policy);
        assert!(!fixture.emitter.literal_delivery());
        assert!(fixture.emitter.terminal_formatter_request().is_none());
        assert!(!fixture.raw.is_empty());
        *fixture.recorder.transcript_buffer_handle().lock().await = fixture.raw.clone();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        fixture.recorder.bind_test_live_finals(1, closed_rx);
        let mut take = fixture.recorder.detach_closed_take(false);
        assert_eq!(take.capture_identity(), (Some(TAKE), 1));
        fixture.emitter.on_formatter_pending(true);
        let emitter = Arc::clone(&fixture.emitter);
        let source = fixture.sources[0].clone();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            release_rx.await.unwrap();
            Fixture::propose_for_policy(&emitter, source, "Poprawione zdanie.", policy);
            emitter.on_formatter_pending(false);
        });
        let painted = fixture.emitter.begin_stop_canvas();
        // A real completion receiver on the original ClosedTake, not the blank
        // recorder's missing-receiver path that would already defer on baseline.
        closed_tx.send(()).unwrap();
        let stopped_at = std::time::Instant::now();
        let armed_order = take.seal_lane_armed();
        let wait = await_live_finals_for_delivery(
            async {
                let admitted = take.wait_live_finals_admitted().await;
                fixture.emitter.wait_paint_published().await && admitted
            },
            || fixture.emitter.finish_stop_canvas(),
            tokio::time::Instant::from_std(stopped_at),
            painted,
            armed_order,
            None,
        )
        .await;
        assert!(wait.live_finals_admitted);
        assert!(!wait.stop_final_timeout);
        assert!(
            wait.permits_first_delivery(Some(TAKE)),
            "positive baseline control"
        );
        let initial = fixture
            .controller
            .stop_canvas_delivery(
                Some(TAKE),
                Some(&fixture.emitter),
                wait,
                stopped_at,
                (false, CaptureTurnIntent::HandsFree, &settings),
                |_| async { panic!("first Stop froze or delivered a nonterminal prefix") },
            )
            .await;
        assert!(!initial.preempted);
        let initial_delivery = initial.delivery.unwrap();
        assert_eq!(initial_delivery, None);
        assert!(fixture.controller.delivered_take.lock().await.is_none());
        assert!(fixture.emitter.terminal_formatter_request().is_none());

        fixture.emitter.on_event(&EngineEvent::SessionFinalised {
            session_id: TAKE.into(),
            layer_summary: LayerSummary::default(),
        });
        let request = fixture.emitter.terminal_formatter_request().unwrap();
        let mut selecting = Box::pin(fixture.controller.select_paste_projection(
            Some(TAKE),
            &settings,
            request.source_text.clone(),
        ));
        tokio::select! {
            biased;
            selected = &mut selecting => panic!("selected before formatter arrived: {selected}"),
            () = tokio::task::yield_now() => {}
        }
        release_tx.send(()).unwrap();
        let selected = selecting.await;
        job.await.unwrap();
        assert_eq!(selected, "Poprawione zdanie.");
        let payloads = std::sync::Mutex::new(Vec::new());
        let config = Config {
            paste_mode: PasteMode::Comfort,
            quick_notes_enabled: false,
            transcript_tagging_enabled: false,
            ..settings.values().clone()
        };
        let delivered = deliver_terminal_unless_settled(initial_delivery, || async {
            fixture
                .controller
                .deliver_stop_transcript_with_sink(
                    Some(TAKE),
                    &selected,
                    (false, false, CaptureTurnIntent::HandsFree, false),
                    &config,
                    |_, payload, _| {
                        payloads.lock().unwrap().push(payload);
                        std::future::ready(Ok(OverlayPasteResult {
                            delivery: OverlayPasteDelivery::PasteRequested,
                            target_app_name: None,
                            frontmost_app_name: None,
                            deferred_insert_shortcut: None,
                            deferred_insert_failure: None,
                        }))
                    },
                )
                .await
        })
        .await
        .unwrap();
        assert_eq!(delivered, TranscriptDelivery::SinkAccepted);
        deliver_terminal_unless_settled(Some(delivered), || async {
            panic!("terminal settlement delivered twice")
        })
        .await
        .unwrap();
        assert_eq!(
            payloads.lock().unwrap().as_slice(),
            std::slice::from_ref(&selected)
        );
        let projection = fixture
            .emitter
            .delivery_projection(policy, &request.source_text)
            .unwrap();
        assert_eq!(projection.status, "applied");
        assert_eq!(projection.delivered_mode, policy.as_str());
        let deliveries: Vec<serde_json::Value> = std::fs::read_to_string(&fixture.path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|row: &serde_json::Value| row["schema"] == "codescribe.projection-delivery.v1")
            .collect();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(
            deliveries[0]["source_raw_revision"],
            request.source_revision
        );
        assert_eq!(deliveries[0]["selected_mode"], policy.as_str());
        assert_eq!(deliveries[0]["projection_receipt"], projection.receipt_id);
        assert_eq!(deliveries[0]["payload_utf8"], selected);
    }
}
