//! Deterministic single-shot roundtrip through the real agent engine path — the
//! same `create_provider_for_lane()` the Swift chat send uses via the bridge.
//!
//! The test owns an isolated config directory and a local Responses endpoint,
//! so it runs in the ordinary workspace gate without a real API key. Run with:
//!
//! ```bash
//! cargo test --test e2e_agent_lane_roundtrip -- --nocapture
//! ```
//!
//! This is the regression net for the "I can't reach the model yet" loop: the
//! lane must resolve from CURRENT settings (lane_truth), a key-optional Custom
//! provider must stream without auth headers, and the turn must finish cleanly.

use codescribe::agent::create_provider_for_lane;
use codescribe_core::agent::{AgentEvent, ContentBlock, Message, Role, StreamOptions};
use codescribe_core::config::{Config, UserSettings};
use codescribe_core::llm::provider::{CustomProvider, WireFamily};
use mockito::Matcher;
use serial_test::serial;
use tempfile::TempDir;

#[tokio::test]
#[serial]
async fn assistive_lane_answers_one_single_shot_turn() {
    selected_agent_lane_roundtrip(codescribe_core::config::RuntimeLlmLaneKind::Assistive).await;
}

#[tokio::test]
#[serial]
async fn max_uses_formatting_provider_and_model_not_chat_settings() {
    selected_agent_lane_roundtrip(codescribe_core::config::RuntimeLlmLaneKind::Formatting).await;
}

async fn selected_agent_lane_roundtrip(lane: codescribe_core::config::RuntimeLlmLaneKind) {
    let data_dir = TempDir::new().expect("isolated Codescribe data directory");
    let mut server = mockito::Server::new_async().await;
    let endpoint = format!("{}/v1/responses", server.url());
    let response_body = [
        r#"data: {"type":"response.created","response":{"id":"resp_fixture"}}"#,
        "",
        r#"data: {"type":"response.output_text.delta","delta":"pong"}"#,
        "",
        r#"data: {"type":"response.output_text.done","text":"pong"}"#,
        "",
        r#"data: {"type":"response.completed","response":{"id":"resp_fixture","status":"completed"}}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n");
    let mock = server
        .mock("POST", "/v1/responses")
        .match_header("authorization", Matcher::Missing)
        .match_header("x-api-key", Matcher::Missing)
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("fixture-model".to_string()),
            Matcher::Regex("Reply with the single word: pong".to_string()),
        ]))
        .match_request(|request| {
            let body: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            !body["instructions"]
                .as_str()
                .unwrap_or("")
                .starts_with("Classify whether")
        })
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(response_body)
        .expect(
            if lane == codescribe_core::config::RuntimeLlmLaneKind::Formatting {
                2
            } else {
                1
            },
        )
        .create_async()
        .await;

    let _data_dir = EnvGuard::set(
        "CODESCRIBE_DATA_DIR",
        data_dir.path().to_string_lossy().as_ref(),
    );
    let _disable_keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    let _provider = EnvGuard::remove("LLM_ASSISTIVE_PROVIDER");
    let _model = EnvGuard::remove("LLM_ASSISTIVE_MODEL");
    let _formatting_provider = EnvGuard::remove("LLM_FORMATTING_PROVIDER");
    let _formatting_model = EnvGuard::remove("LLM_FORMATTING_MODEL");
    let _level = EnvGuard::remove("FORMATTING_LEVEL");
    // The mock host is a Custom provider row (no key required); the lane
    // points at it through settings.json, the same path the Settings UI takes.
    let row = CustomProvider::new("Fixture Box", WireFamily::OpenAiResponses, &endpoint)
        .expect("valid custom row");
    let mut settings = UserSettings::default();
    settings.add_custom_provider(row).expect("add custom row");
    let other = CustomProvider::new(
        "Wrong Lane",
        WireFamily::OpenAiResponses,
        &format!("{}/must-not-reach/v1/responses", server.url()),
    )
    .expect("distinct unselected endpoint");
    settings
        .add_custom_provider(other)
        .expect("add unselected row");
    settings.llm_assistive_provider = Some("custom:wrong-lane".into());
    settings.llm_assistive_model = Some("wrong-model".into());
    settings.llm_formatting_provider = Some("custom:wrong-lane".into());
    settings.llm_formatting_model = Some("wrong-model".into());
    match lane {
        codescribe_core::config::RuntimeLlmLaneKind::Assistive => {
            settings.llm_assistive_provider = Some("custom:fixture-box".into());
            settings.llm_assistive_model = Some("fixture-model".into());
        }
        codescribe_core::config::RuntimeLlmLaneKind::Formatting => {
            settings.llm_formatting_provider = Some("custom:fixture-box".into());
            settings.llm_formatting_model = Some("fixture-model".into());
        }
    }
    settings.save().expect("persist lane settings");
    Config::default()
        .save_to_env("FORMATTING_LEVEL", "max")
        .expect("enable Max");
    let _attempt_timeout = EnvGuard::set("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
    let _chunk_timeout = EnvGuard::set("CODESCRIBE_AI_INTER_CHUNK_TIMEOUT_MS", "2000");

    let runtime_settings = Config::load_runtime_snapshot().expect("seal runtime settings");
    assert_eq!(
        runtime_settings
            .ai_execution()
            .request_timing()
            .attempt_timeout()
            .as_millis(),
        2_000
    );
    assert_eq!(
        runtime_settings
            .ai_execution()
            .request_timing()
            .inter_chunk_timeout()
            .as_millis(),
        2_000
    );
    if lane == codescribe_core::config::RuntimeLlmLaneKind::Formatting {
        use codescribe_core::agent::{ThreadDeliveryGateway, ToolRegistry};
        use std::sync::Arc;
        let consultation = codescribe::agent::max_consultation::MaxConsultation::start(
            "max-http-fixture".into(),
            &runtime_settings,
            Arc::new(ToolRegistry::new()),
            None,
            ThreadDeliveryGateway::new_in(data_dir.path().join("threads")).expect("gateway"),
            Arc::new(|_, _, _| {}),
            data_dir.path().join("agent-turn.lock"),
        )
        .expect("Max host starts");
        let result = consultation
            .enqueue(
                "first".into(),
                "Reply with the single word: pong".into(),
                Vec::new(),
                &runtime_settings,
            )
            .expect("admitted")
            .await
            .expect("owner response")
            .expect("durable Max answer");
        assert_eq!(result.text, "pong");
        assert_eq!(result.delivery.message_count, 2);
        assert_eq!(result.delivery.backend_id, consultation.id());

        // Exercise the group result through the public host capability and
        // actual presentation owner, not a hand-built presentation receipt.
        use codescribe::presentation::{
            PresentationEmitter, TranscriptBus, TranscriptMode, TranscriptSession,
        };
        use codescribe_core::agent::consultation::SealedConsultationInput;
        use codescribe_core::ai_formatting::FormattingAgent;
        use codescribe_core::audio::capture_receipt::{
            AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
        };
        use codescribe_core::pipeline::acoustic_ledger::{
            AcousticLedger, ObservationProducer, OccurrenceIdentity,
        };
        use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};
        use codescribe_core::stt::tail_provider::TailSampleRange;
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let delivery = Arc::new(tokio::sync::Mutex::new(String::new()));
        let bus_path = data_dir.path().join("group-bus.jsonl");
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "http-capture".into(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                },
                bus_path.clone(),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let emitter = PresentationEmitter::new_with_authority(
            Arc::clone(&delivery),
            None,
            None,
            Some(bus),
            Some(Arc::clone(&ledger)),
            None,
        );
        for (index, label) in ["Reply with the single word:", "pong"]
            .into_iter()
            .enumerate()
        {
            let occurrence = OccurrenceIdentity::new(
                "http-capture",
                1,
                index as u64 * 16_000,
                (index as u64 + 1) * 16_000,
            );
            let (mutation, seal) = {
                let mut ledger = ledger.lock().unwrap();
                let mutation =
                    group_fixture_mutation(&mut ledger, occurrence.clone(), index as u64, label);
                ledger.schedule_frontier(occurrence.clone(), [ObservationProducer::Apple]);
                ledger.note_frontier_return(&occurrence, ObservationProducer::Apple);
                (mutation, ledger.seal(&occurrence).unwrap().clone())
            };
            emitter.on_event(&mutation);
            emitter.on_event(&EngineEvent::LedgerSeal { receipt: seal });
        }
        let speech = AcousticSpeechEvidence::measured(
            CaptureEvidenceIdentity::new("http-capture", 1),
            "http-fixture",
            AcousticAvailability::Observed {
                observed_samples: 32_000,
            },
            vec![TailSampleRange {
                session: "http-capture".into(),
                capture_epoch: 1,
                sample_start: 0,
                sample_end: 32_000,
            }],
        );
        let input = SealedConsultationInput::from_ledger(
            &ledger.lock().unwrap(),
            "http-capture",
            1,
            0..32_000,
            &speech,
        )
        .unwrap()
        .unwrap();

        // Assess through the same concrete HTTP provider, without admitting
        // an execution turn. The following group still becomes history turn 2.
        let assessment_response = [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"COMPLETE\"}",
            "",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"COMPLETE\"}",
            "",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"assessment\",\"status\":\"completed\"}}",
            "",
            "data: [DONE]",
            "",
        ].join("\n");
        let expected_text = input.text().to_string();
        let assessment_mock = server
            .mock("POST", "/v1/responses")
            .match_header("authorization", Matcher::Missing)
            .match_request(move |request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body().unwrap()).unwrap();
                body["model"] == "fixture-model"
                    && body["instructions"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with("Classify whether")
                    && body.get("tools").is_none()
                    && body.get("previous_response_id").is_none()
                    && body["max_output_tokens"] == 64
                    && body["input"].as_array().is_some_and(|messages| {
                        messages.len() == 1
                            && messages[0]["content"][0]["text"]
                                .as_str()
                                .is_some_and(|text| {
                                    serde_json::from_str::<serde_json::Value>(text)
                                        .is_ok_and(|input| input["transcript"] == expected_text)
                                })
                    })
            })
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(assessment_response)
            .expect(1)
            .create_async()
            .await;
        match consultation
            .assess_group(input.clone(), &runtime_settings)
            .await
            .unwrap()
        {
            codescribe_core::agent::consultation::ConsultationReadiness::Complete(assessed) => {
                assert_eq!(assessed, input);
            }
            codescribe_core::agent::consultation::ConsultationReadiness::Continue(_) => {
                panic!("clean COMPLETE fixture must retain the assessed input");
            }
        }
        assessment_mock.assert_async().await;
        let inspection = ThreadDeliveryGateway::new_in(data_dir.path().join("threads"))
            .unwrap()
            .inspect_consultation(consultation.id())
            .unwrap()
            .unwrap();
        assert!(inspection.pending_turn_id.is_none());
        assert!(inspection.retained_inputs.is_empty());
        let pending = consultation
            .prepare_group(input.clone(), &runtime_settings)
            .unwrap()
            .authorize()
            .unwrap();
        // The destination advances before applying the answer. Its open suffix
        // is not part of the Agent request and must remain untouched.
        let later = group_fixture_mutation(
            &mut ledger.lock().unwrap(),
            OccurrenceIdentity::new("http-capture", 1, 32_000, 48_000),
            2,
            "later words",
        );
        emitter.on_event(&later);
        let completed = pending.finish().await.unwrap();
        assert_eq!(completed.answer().delivery.message_count, 4);
        assert_eq!(completed.input(), &input);
        let mut closed = PresentationEmitter::new_with_authority(
            Arc::new(tokio::sync::Mutex::new(String::new())),
            None,
            None,
            None,
            Some(Arc::clone(&ledger)),
            None,
        );
        closed.finish().await;
        assert_eq!(
            closed.apply_consultation_presentation(&completed),
            Err(
                codescribe::presentation::emitter::UserRevisionRefusal::LedgerRefusal(
                    "consultation_delivery_closed"
                )
            )
        );
        assert!(
            ledger
                .lock()
                .unwrap()
                .consultation_presentations()
                .is_empty()
        );
        let mut emitter = Arc::new(emitter);
        let observer = Arc::new(codescribe_core::pipeline::sinks::CollectorEventSink::new());
        let sink = codescribe_core::pipeline::sinks::FanoutEventSink::pair(
            observer.clone(),
            emitter.clone(),
        );
        assert_eq!(emitter.consultation_destinations(), 1);
        assert!(closed.on_consultation_completed(&completed).is_err());
        sink.on_consultation_completed(&completed).unwrap();
        assert!(sink.on_consultation_completed(&completed).is_err());
        assert!(observer.events().is_empty());
        assert_eq!(ledger.lock().unwrap().consultation_presentations().len(), 1);
        drop(sink);
        Arc::get_mut(&mut emitter).unwrap().finish().await;
        assert_eq!(delivery.lock().await.as_str(), "pong later words");
        let rows = std::fs::read_to_string(bus_path).unwrap();
        let row = rows
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .rfind(|row| row["reducer_action"] == "apply_consultation_presentation")
            .unwrap();
        assert_eq!(row["rendered_text"], "pong later words");
        assert_eq!(
            row["consultation_presentations"][0]["consultation_id"],
            consultation.id()
        );
        assert_eq!(
            row["consultation_presentations"][0]["turn_id"],
            input.turn_id_for_group()
        );
        assert_eq!(
            row["consultation_presentations"][0]["members"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        consultation.close_if_idle().await.unwrap();
        mock.assert_async().await;
        return;
    }
    let provider = create_provider_for_lane(&runtime_settings, lane)
        .expect("selected lane must be available (see the reported reason)");

    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::Text(
            "Reply with the single word: pong".to_string(),
        )],
    )];
    let options = StreamOptions {
        model: String::new(),
        system_prompt: None,
        max_tokens: Some(32),
        temperature: None,
        reset_chain: false,
    };

    let mut rx = provider
        .stream(&messages, &[], &options)
        .await
        .expect("stream must start");

    let mut text = String::new();
    let mut clean_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::TextDelta(delta) => text.push_str(&delta),
            AgentEvent::TextDone(done) if !done.trim().is_empty() => text = done,
            AgentEvent::ResponseDone { clean, .. } => clean_done = clean,
            AgentEvent::Error(error) => panic!("provider error: {error}"),
            _ => {}
        }
    }

    assert!(clean_done, "turn must end on a clean terminal");
    assert_eq!(text.trim(), "pong");
    mock.assert_async().await;
    eprintln!("agent replied: {text}");
}

/// Synthetic acoustic evidence only; this does not claim a microphone roundtrip.
fn group_fixture_mutation(
    ledger: &mut codescribe_core::pipeline::acoustic_ledger::AcousticLedger,
    occurrence: codescribe_core::pipeline::acoustic_ledger::OccurrenceIdentity,
    request: u64,
    label: &str,
) -> codescribe_core::pipeline::contracts::EngineEvent {
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, EnergyCalibration, ObservationIdentity, ObservationProducer,
    };
    let calibration = EnergyCalibration::new("http-fixture", 1.0, 1);
    let evidence = AcousticEvidence {
        occurrence: occurrence.clone(),
        duration_ms: 1_000.0,
        energy_integral: 10.0,
        mean_rms_dbfs: -12.0,
        peak_dbfs: -3.0,
        vad_open_sample: Some(occurrence.sample_start),
        vad_close_sample: Some(occurrence.sample_end),
        evidence_calibration_version: calibration.version.clone(),
    };
    assert!(ledger.qualify(&evidence, &calibration).is_qualified());
    let observation = ObservationIdentity::new(ObservationProducer::Apple, request, 0, occurrence);
    let receipt = ledger.admit(&observation, label);
    assert!(receipt.grants_mutation());
    codescribe_core::pipeline::contracts::EngineEvent::LedgerMutation {
        observation,
        label: label.to_string(),
        receipt,
    }
}

#[tokio::test]
#[serial]
async fn formatting_agent_provider_requires_max_without_disabling_chat() {
    use codescribe_core::config::{FormattingPolicy, RuntimeLlmLaneKind};

    let data_dir = TempDir::new().expect("isolated Max settings");
    let _data_dir = EnvGuard::set(
        "CODESCRIBE_DATA_DIR",
        data_dir.path().to_string_lossy().as_ref(),
    );
    let _keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
    let _level = EnvGuard::remove("FORMATTING_LEVEL");
    let _formatting = EnvGuard::remove("LLM_FORMATTING_PROVIDER");
    let _assistive = EnvGuard::remove("LLM_ASSISTIVE_PROVIDER");
    let server = mockito::Server::new_async().await;
    let row = CustomProvider::new(
        "Max Fixture",
        WireFamily::OpenAiResponses,
        &format!("{}/v1/responses", server.url()),
    )
    .expect("fixture provider");
    let mut settings = UserSettings::default();
    settings.add_custom_provider(row).expect("register fixture");
    settings.llm_formatting_provider = Some("custom:max-fixture".into());
    settings.llm_assistive_provider = Some("custom:max-fixture".into());
    settings.save().expect("save fixture lanes");

    for policy in FormattingPolicy::ALL {
        Config::default()
            .save_to_env("FORMATTING_LEVEL", policy.as_str())
            .expect("persist formatting policy");
        let snapshot = Config::load_runtime_snapshot().expect("seal policy");
        assert_eq!(snapshot.formatting_policy(), policy);
        let formatting = create_provider_for_lane(&snapshot, RuntimeLlmLaneKind::Formatting);
        if policy == FormattingPolicy::Max {
            assert!(formatting.is_ok(), "Max must admit its configured provider");
        } else {
            let error = formatting
                .err()
                .expect("non-Max must refuse before provider construction");
            assert!(error.to_string().contains("requires Max policy"));
        }
        assert!(
            create_provider_for_lane(&snapshot, RuntimeLlmLaneKind::Assistive).is_ok(),
            "formatting policy must not disable the independent Agent chat"
        );
    }
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: this process-environment test is serialized.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: this process-environment test is serialized.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: this process-environment test is serialized.
        unsafe {
            match self.previous.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
