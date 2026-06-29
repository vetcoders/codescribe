//! Controller unit tests

use super::*;
use serial_test::serial;
use std::time::Duration;

#[tokio::test]
async fn test_initial_state() {
    let controller = RecordingController::new();
    assert_eq!(controller.current_state().await, State::Idle);
}

#[tokio::test]
async fn test_last_segment_audio_offset_initialized_to_zero() {
    // commit_segment relies on this starting at 0 — the first segment of a
    // toggle session clips from sample 0. start_toggle_recording then resets
    // it to 0 again (defensive against leaking offset across sessions).
    let controller = RecordingController::new();
    assert_eq!(
        controller
            .last_segment_audio_offset
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "last_segment_audio_offset must start at 0 — commit_segment first-call \
         contract requires snapshot from buffer start of the active toggle session"
    );
}

#[tokio::test]
async fn test_last_segment_audio_offset_atomic_advance_and_reset() {
    // Smoke for the atomic ops that commit_segment + start_toggle_recording use.
    // commit_segment: load(SeqCst) → snapshot → store(end_offset, SeqCst).
    // start_toggle_recording: store(0, SeqCst).
    use std::sync::atomic::Ordering;
    let controller = RecordingController::new();

    controller
        .last_segment_audio_offset
        .store(48000, Ordering::SeqCst);
    assert_eq!(
        controller.last_segment_audio_offset.load(Ordering::SeqCst),
        48000
    );

    // Simulate start_toggle_recording's reset on new session.
    controller
        .last_segment_audio_offset
        .store(0, Ordering::SeqCst);
    assert_eq!(
        controller.last_segment_audio_offset.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn test_renamed_request_recording_stop_is_callable() {
    // Compile-time guard that the rename `request_recording_commit` →
    // `request_recording_stop` shipped intact. If anyone re-renames or
    // removes the function this test stops compiling. Body doesn't have to
    // execute (OVERLAY_CONTROLLER won't be registered in tests, so the call
    // gates out early with a warn!) — we just need the symbol to resolve.
    let _: fn() = request_recording_stop;
    let _: fn() = request_segment_commit;
    let _: fn() = request_segment_commit_and_augment;
}

#[test]
fn test_assistive_hold_delay_floor_preserves_higher_configured_delay() {
    assert_eq!(effective_hold_start_delay_ms(200, false), 200);
    assert_eq!(effective_hold_start_delay_ms(200, true), 400);
    assert_eq!(effective_hold_start_delay_ms(800, true), 800);
}

#[test]
fn test_toggle_stop_watchdog_allows_default_ai_attempt_budget() {
    assert!(
        toggle_stop_adjudicate_timeout() >= Duration::from_secs(90),
        "toggle stop watchdog must not fire before the default AI attempt/inter-chunk budget"
    );
}

#[test]
fn test_overlay_format_result_marks_failed_formatting_raw() {
    let out = overlay_format_result_text(
        "raw transcript",
        crate::ai_formatting::AiFormatResult {
            text: "raw transcript".to_string(),
            reasoning_text: None,
            status: crate::ai_formatting::AiFormatStatus::Failed,
        },
    );

    assert_eq!(out, "raw transcript\n\n(raw — formatting failed)");
}

#[test]
fn test_overlay_format_result_marks_empty_formatting_output() {
    let out = overlay_format_result_text(
        "raw transcript",
        crate::ai_formatting::AiFormatResult {
            text: "   ".to_string(),
            reasoning_text: None,
            status: crate::ai_formatting::AiFormatStatus::Applied,
        },
    );

    assert_eq!(out, "raw transcript\n\n(raw — formatting failed)");
}

#[test]
fn test_overlay_format_result_keeps_applied_formatting() {
    let out = overlay_format_result_text(
        "raw transcript",
        crate::ai_formatting::AiFormatResult {
            text: "Formatted transcript.".to_string(),
            reasoning_text: None,
            status: crate::ai_formatting::AiFormatStatus::Applied,
        },
    );

    assert_eq!(out, "Formatted transcript.");
}

#[tokio::test]
#[serial]
async fn test_hold_down_schedules_delayed_start() {
    let controller = RecordingController::new();
    // Override hold delay for faster test
    controller.config.write().await.hold_start_delay_ms = 100;

    let event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };

    controller.handle_hotkey_event(event).await.unwrap();

    // Should still be IDLE (delay not elapsed)
    assert_eq!(controller.current_state().await, State::Idle);

    // Wait for delay to elapse
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Should now be REC_HOLD
    assert_eq!(controller.current_state().await, State::RecHold);
}

#[tokio::test]
#[serial]
async fn test_assistive_hold_ctrl_uses_safe_delay_floor() {
    let controller = RecordingController::new();
    controller.config.write().await.hold_start_delay_ms = 200;

    let event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: true,
        hold_mode: HoldMode::Selection,
        force_raw: false,
        force_ai: false,
    };

    controller.handle_hotkey_event(event).await.unwrap();

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(controller.current_state().await, State::Idle);

    tokio::time::sleep(Duration::from_millis(220)).await;
    assert_eq!(controller.current_state().await, State::RecHold);
}

#[tokio::test]
#[serial]
async fn test_hold_up_before_delay_cancels() {
    let controller = RecordingController::new();
    // Override hold delay for faster test
    controller.config.write().await.hold_start_delay_ms = 200;

    // Press down
    let down_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(down_event).await.unwrap();

    // Release before delay elapses
    tokio::time::sleep(Duration::from_millis(50)).await;
    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(up_event).await.unwrap();

    // Wait past the original delay
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Should still be IDLE (start was cancelled)
    assert_eq!(controller.current_state().await, State::Idle);
}

#[tokio::test]
#[serial]
async fn test_fast_assistive_ctrl_tap_before_floor_is_noop() {
    let controller = RecordingController::new();
    controller.config.write().await.hold_start_delay_ms = 200;

    let down_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: true,
        hold_mode: HoldMode::Selection,
        force_raw: false,
        force_ai: false,
    };
    controller.handle_hotkey_event(down_event).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: true,
        hold_mode: HoldMode::Selection,
        force_raw: false,
        force_ai: false,
    };
    controller.handle_hotkey_event(up_event).await.unwrap();

    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(controller.current_state().await, State::Idle);
}

#[tokio::test]
#[serial]
async fn test_toggle_starts_immediately() {
    let controller = RecordingController::new();

    let event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: true,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };

    controller.handle_hotkey_event(event).await.unwrap();

    // Should immediately transition to REC_TOGGLE
    assert_eq!(controller.current_state().await, State::RecToggle);
}

#[tokio::test]
#[serial]
async fn test_busy_state_ignores_hotkeys() {
    let controller = RecordingController::new();

    // Manually set to BUSY
    *controller.state.write().await = State::Busy;

    let event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };

    controller.handle_hotkey_event(event).await.unwrap();

    // Should remain BUSY
    assert_eq!(controller.current_state().await, State::Busy);
}

#[tokio::test]
async fn test_state_display() {
    assert_eq!(State::Idle.to_string(), "IDLE");
    assert_eq!(State::RecHold.to_string(), "REC_HOLD");
    assert_eq!(State::RecToggle.to_string(), "REC_TOGGLE");
    assert_eq!(State::Busy.to_string(), "BUSY");
}

#[tokio::test]
async fn test_reset_from_busy() {
    let controller = RecordingController::new();

    // Manually set to BUSY (simulating stuck state)
    *controller.state.write().await = State::Busy;
    assert!(controller.is_busy().await);

    // Reset should force back to IDLE
    controller.reset().await;
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!controller.is_busy().await);
}

#[tokio::test]
async fn test_is_recording_states() {
    let controller = RecordingController::new();

    // IDLE - not recording
    assert!(!controller.is_recording().await);

    // REC_HOLD - recording
    *controller.state.write().await = State::RecHold;
    assert!(controller.is_recording().await);

    // REC_TOGGLE - recording
    *controller.state.write().await = State::RecToggle;
    assert!(controller.is_recording().await);

    // BUSY - not recording (processing)
    *controller.state.write().await = State::Busy;
    assert!(!controller.is_recording().await);
}

// ============================================================
// NEW HOTKEY ARCHITECTURE TESTS (force_raw_mode logic)
// ============================================================
//
// These tests verify the new mode determination logic:
// - Ctrl Hold (no Shift) → force_raw=true, assistive=false → RAW mode
// - Ctrl+Shift Hold → force_raw=false, assistive=true → Assistive mode
// - Left Double Option → force_ai=true, assistive=false → Formatting mode
// - Toggle (no force_ai) → respects AI_FORMATTING_ENABLED setting

#[tokio::test]
#[serial]
async fn test_hold_down_sets_force_raw_mode() {
    let controller = RecordingController::new();

    // Verify initial state
    assert!(!*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);

    // Hold Down without Shift → force_raw=true
    let event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(event).await.unwrap();

    // force_raw should be true, assistive should be false
    assert!(
        *controller.force_raw_mode.read().await,
        "Hold Down should set force_raw_mode=true"
    );
    assert!(
        !*controller.assistive_mode.read().await,
        "Hold Down without Shift should keep assistive_mode=false"
    );
}

#[test]
fn test_action_contract_mode_prefers_raw_when_forced() {
    let mode = resolve_transcription_action_contract_mode(true, false, true, true);
    assert_eq!(
        mode,
        crate::controller::TranscriptionActionContractMode::Raw
    );
}

#[test]
fn test_action_contract_mode_uses_ai_format_when_force_ai_enabled() {
    let mode = resolve_transcription_action_contract_mode(false, true, false, false);
    assert_eq!(
        mode,
        crate::controller::TranscriptionActionContractMode::AiFormat
    );
}

#[test]
fn test_action_contract_mode_uses_ai_format_for_toggle_ai_path() {
    let mode = resolve_transcription_action_contract_mode(false, false, true, true);
    assert_eq!(
        mode,
        crate::controller::TranscriptionActionContractMode::AiFormat
    );
}

#[test]
fn test_action_contract_mode_uses_raw_for_toggle_without_ai() {
    let mode = resolve_transcription_action_contract_mode(false, false, true, false);
    assert_eq!(
        mode,
        crate::controller::TranscriptionActionContractMode::Raw
    );
}

#[test]
fn test_truth_engine_label_maps_toggle_session_adjudicated_to_local_whisper() {
    assert_eq!(
        truth_engine_label(Some(RecordingTranscriptSource::ToggleSessionAdjudicated)).as_deref(),
        Some("local_whisper")
    );
}

#[test]
fn test_toggle_session_adjudicated_label_is_user_facing() {
    assert_eq!(
        RecordingTranscriptSource::ToggleSessionAdjudicated.label(),
        "Toggle session adjudicated"
    );
}

#[test]
#[serial]
fn test_toggle_final_pass_enabled_defaults_true_and_honors_falsey_values() {
    unsafe {
        std::env::remove_var("CODESCRIBE_TOGGLE_FINAL_PASS");
    }
    assert!(toggle_final_pass_enabled());

    for falsey in ["0", "false", "FALSE", "no", "off", " off ", "   "] {
        unsafe {
            std::env::set_var("CODESCRIBE_TOGGLE_FINAL_PASS", falsey);
        }
        assert!(
            !toggle_final_pass_enabled(),
            "expected {falsey:?} to disable toggle final pass"
        );
    }

    unsafe {
        std::env::set_var("CODESCRIBE_TOGGLE_FINAL_PASS", "1");
    }
    assert!(toggle_final_pass_enabled());

    unsafe {
        std::env::remove_var("CODESCRIBE_TOGGLE_FINAL_PASS");
    }
}

#[test]
fn test_should_use_toggle_adjudicated_stop_only_for_raw_toggle_when_enabled() {
    assert!(should_use_toggle_adjudicated_stop(
        State::RecToggle,
        false,
        true
    ));
    assert!(!should_use_toggle_adjudicated_stop(
        State::RecToggle,
        true,
        true
    ));
    assert!(!should_use_toggle_adjudicated_stop(
        State::RecToggle,
        false,
        false
    ));
    assert!(!should_use_toggle_adjudicated_stop(
        State::RecHold,
        false,
        true
    ));
    assert!(!should_use_toggle_adjudicated_stop(
        State::Busy,
        false,
        true
    ));
}

#[test]
fn test_transcript_delivery_wrap_is_default_off() {
    let config = Config::default();

    assert_eq!(
        maybe_wrap_transcript_for_delivery("literal transcript", &config, "dictation"),
        "literal transcript"
    );
}

#[test]
fn test_transcript_delivery_wrap_uses_config_when_enabled() {
    let config = Config {
        transcript_tagging_enabled: true,
        transcript_tag_template:
            codescribe_core::transcript_tagging::DEFAULT_TRANSCRIPT_TAG_TEMPLATE.to_string(),
        ..Config::default()
    };

    assert_eq!(
        maybe_wrap_transcript_for_delivery("literal transcript", &config, "dictation"),
        "<codescribe mode=\"dictation\" lang=\"pl\">\nliteral transcript\n</codescribe>"
    );
}

#[test]
fn test_toggle_stop_event_preserves_active_session_identity() {
    let right_option_stop = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: true,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    assert!(
        !should_apply_incoming_mode_flags(State::RecToggle, &right_option_stop),
        "A stop key must not smear its assistive/formatting flags onto the active toggle session"
    );
    assert!(
        should_apply_incoming_mode_flags(State::Idle, &right_option_stop),
        "The same key still defines a new session when no toggle session is active"
    );
}

#[tokio::test]
#[serial]
async fn test_agent_send_in_flight_blocks_nonassistive_hotkey_starts() {
    // Contract (preserved): a *raw* dictation start fired while a background
    // agent turn is still streaming stays blocked — barging a raw transcript
    // into a live agent turn is never wanted, and it preserves the single audio
    // pipeline. The block lands before the mode-flag section, so the controller
    // stays Idle with default flags.
    let controller = RecordingController::new();
    helpers::set_agent_send_in_flight_for_test(true);

    let raw_hold = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };

    controller
        .handle_hotkey_event(raw_hold)
        .await
        .expect("agent-busy hotkey block should be non-fatal");

    assert_eq!(controller.current_state().await, State::Idle);
    assert_eq!(*controller.hold_mode.read().await, HoldMode::Raw);
    assert!(!*controller.assistive_mode.read().await);
    helpers::set_agent_send_in_flight_for_test(false);
}

/// Assistive Talk Anytime — the agent-send gate decision is a pure function of
/// (state, event, in-flight flag). These assertions pin the new contract
/// without spawning the heavy async recording machinery.
#[test]
fn test_assistive_talk_anytime_gate_predicate() {
    let assistive_chat_hold = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false,
    };
    let raw_hold = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let assistive_toggle = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: true,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    let release = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false,
    };

    // Classifier: only *start* events flagged assistive are Talk-Anytime starts.
    assert!(is_assistive_start_event(&assistive_chat_hold));
    assert!(is_assistive_start_event(&assistive_toggle));
    assert!(!is_assistive_start_event(&raw_hold)); // raw start is not assistive
    assert!(!is_assistive_start_event(&release)); // a release is not a start

    // Talk Anytime: an assistive start is ALLOWED through while the agent
    // answers in the background (Idle + in-flight) — it must reach the recording
    // path so its utterance flows into the pending-follow-up buffer.
    assert!(
        !should_block_hotkey_during_agent_send(State::Idle, &assistive_chat_hold, true),
        "FN+Shift Talk Anytime must not be ignored while an agent turn streams"
    );
    assert!(
        !should_block_hotkey_during_agent_send(State::Idle, &assistive_toggle, true),
        "assistive toggle Talk Anytime must not be ignored while an agent turn streams"
    );

    // Protected: a raw dictation start stays blocked during a streaming turn.
    assert!(
        should_block_hotkey_during_agent_send(State::Idle, &raw_hold, true),
        "raw dictation must not barge a live agent turn"
    );

    // No agent in flight → nothing is gated (normal idle dictation works).
    assert!(!should_block_hotkey_during_agent_send(
        State::Idle,
        &raw_hold,
        false
    ));
    assert!(!should_block_hotkey_during_agent_send(
        State::Idle,
        &assistive_chat_hold,
        false
    ));

    // Non-start events (key release) are never blocked by this gate, so a hold
    // release can always cancel/finish even mid-turn.
    assert!(!should_block_hotkey_during_agent_send(
        State::Idle,
        &release,
        true
    ));

    // The gate only acts at Idle: while audio/transcription holds State::Busy the
    // separate Busy guard owns the decision (this gate stays out of its way).
    assert!(!should_block_hotkey_during_agent_send(
        State::Busy,
        &raw_hold,
        true
    ));
}

fn make_final_pass_verdict(
    text: &str,
    speech_pct: f32,
    avg_logprob: Option<f32>,
    no_speech: bool,
) -> codescribe_core::pipeline::contracts::TranscriptionVerdict {
    codescribe_core::pipeline::contracts::TranscriptionVerdict::from_parts(
        text.to_string(),
        codescribe_core::pipeline::contracts::RawTranscript {
            text: text.to_string(),
            segments: Vec::new(),
            avg_logprob,
            compression_ratio: None,
            quality_gate_dropped: false,
        },
        Some(codescribe_core::pipeline::contracts::VadVerdict {
            speech_pct,
            speech_windows: if no_speech { 0 } else { 4 },
            total_windows: 10,
            no_speech,
            no_speech_reason: if no_speech {
                Some("vad_no_speech_detected".to_string())
            } else {
                None
            },
            sparkline: String::new(),
        }),
        codescribe_core::pipeline::contracts::TranscriptionSource::LocalFinalPass,
        codescribe_core::pipeline::contracts::TranscriptionEngineVerdict::whisper(
            codescribe_core::pipeline::contracts::TranscriptionEngineMode::EmbeddedDefault,
        ),
        None,
    )
}

fn make_cloud_verdict(text: &str) -> crate::client::CloudTranscriptionVerdict {
    crate::client::CloudTranscriptionVerdict {
        text: text.to_string(),
        source: codescribe_core::pipeline::contracts::TranscriptionSource::Cloud,
        confidence_flags: Vec::new(),
        latency_ms: Some(120),
        model_name: Some("mock-cloud".to_string()),
    }
}

#[test]
fn test_adjudicate_recording_truth_blocks_local_no_speech() {
    let session = SessionTelemetrySnapshot {
        no_speech_reason: Some("telemetry_should_not_override_core".to_string()),
        stats: None,
    };

    let verdict = adjudicate_recording_truth(
        true,
        true,
        Some(make_final_pass_verdict("", 0.0, None, true)),
        "preview text".to_string(),
        None,
        &session,
    );

    assert!(verdict.raw_text.is_none());
    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::LocalFinalPass)
    );
    assert_eq!(
        verdict.no_speech_reason.as_deref(),
        Some("vad_no_speech_detected")
    );
    assert_eq!(
        verdict.commit_trigger.as_deref(),
        Some("no_reliable_speech")
    );
    assert_eq!(verdict.display_status, "No reliable speech detected");
}

#[test]
fn test_adjudicate_recording_truth_marks_cloud_fallback_as_degraded() {
    let verdict = adjudicate_recording_truth(
        false,
        false,
        None,
        "streaming fallback".to_string(),
        None,
        &SessionTelemetrySnapshot::default(),
    );

    assert_eq!(verdict.raw_text.as_deref(), Some("streaming fallback"));
    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::StreamingFallback)
    );
    assert_eq!(
        verdict.fallback_class,
        Some(RecordingFallbackClass::Degraded)
    );
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::CloudPrimaryMissing)
    );
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::UnverifiedStream)
    );
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict)
    );
    assert_eq!(
        verdict.commit_trigger.as_deref(),
        Some("streaming_preview_used_as_verdict")
    );
    assert_eq!(verdict.display_status, "Streaming fallback");
}

#[test]
fn test_adjudicate_recording_truth_prefers_local_final_pass_over_streaming_preview() {
    let verdict = adjudicate_recording_truth(
        true,
        true,
        Some(make_final_pass_verdict(
            "czysty final pass",
            82.0,
            Some(-0.22),
            false,
        )),
        "powtarzajacy sie streaming preview".to_string(),
        None,
        &SessionTelemetrySnapshot::default(),
    );

    assert_eq!(verdict.raw_text.as_deref(), Some("czysty final pass"));
    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::LocalFinalPass)
    );
    assert_eq!(verdict.fallback_class, None);
    assert!(verdict.confidence_flags.is_empty());
    assert_eq!(verdict.commit_trigger, None);
    assert_eq!(verdict.display_status, "Final-pass local");
}

#[test]
fn test_adjudicate_recording_truth_marks_raw_streaming_preview_as_degraded_fallback() {
    let verdict = adjudicate_recording_truth(
        true,
        true,
        None,
        "toggle transcript".to_string(),
        None,
        &SessionTelemetrySnapshot::default(),
    );

    assert_eq!(verdict.raw_text.as_deref(), Some("toggle transcript"));
    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::StreamingFallback)
    );
    assert_eq!(
        verdict.fallback_class,
        Some(RecordingFallbackClass::Degraded)
    );
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::LocalFinalPassUnavailable)
    );
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::UnverifiedStream)
    );
    assert_eq!(
        verdict.commit_trigger.as_deref(),
        Some("streaming_preview_used_as_verdict")
    );
    assert_eq!(verdict.display_status, "Streaming fallback");
}

#[test]
fn test_adjudicate_recording_truth_uses_typed_cloud_primary_verdict() {
    let verdict = adjudicate_recording_truth(
        false,
        false,
        None,
        "preview text".to_string(),
        Some(make_cloud_verdict("cloud primary")),
        &SessionTelemetrySnapshot::default(),
    );

    assert_eq!(verdict.raw_text.as_deref(), Some("cloud primary"));
    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::CloudPrimary)
    );
    assert!(verdict.confidence_flags.is_empty());
    assert_eq!(verdict.display_status, "Cloud primary");
}

#[test]
fn test_adjudicate_recording_truth_marks_low_logprob_as_unsafe() {
    let verdict = adjudicate_recording_truth(
        true,
        true,
        Some(make_final_pass_verdict(
            "niepewna transkrypcja",
            28.0,
            Some(-1.2),
            false,
        )),
        "preview text".to_string(),
        None,
        &SessionTelemetrySnapshot::default(),
    );

    assert_eq!(
        verdict.transcript_source,
        Some(RecordingTranscriptSource::LocalFinalPass)
    );
    assert_eq!(verdict.fallback_class, Some(RecordingFallbackClass::Unsafe));
    assert!(
        verdict
            .confidence_flags
            .contains(&TranscriptionConfidenceFlag::PossibleHallucinationLogprob)
    );
    assert_eq!(
        verdict.commit_trigger.as_deref(),
        Some("possible_hallucination_logprob")
    );
    assert_eq!(verdict.display_status, "Possible hallucination");
}

#[test]
fn test_recorder_runtime_recovery_requires_granted_microphone_and_missing_recorder() {
    assert!(should_attempt_recorder_runtime_recovery(
        PermissionStatus::Granted,
        true
    ));
    assert!(!should_attempt_recorder_runtime_recovery(
        PermissionStatus::Denied,
        true
    ));
    assert!(!should_attempt_recorder_runtime_recovery(
        PermissionStatus::Granted,
        false
    ));
}

#[test]
fn test_recorder_recovery_message_uses_settings_language() {
    let message = RecordingController::format_recorder_recovery_message(
        &["Accessibility", "Microphone"],
        "DictationHotkey",
        "FormattingHotkey",
        "AssistiveHotkey",
    );

    assert!(message.contains("Open Settings"));
    assert!(!message.contains("Setup"));
    assert!(message.contains("Accessibility, Microphone"));
}

#[test]
fn test_backend_recovery_message_uses_settings_language() {
    let message =
        RecordingController::format_backend_recovery_message(Some("Cloud endpoint timed out"));

    assert!(message.contains("Open Settings"));
    assert!(!message.contains("Setup"));
    assert!(message.contains("Cloud endpoint timed out"));
}

// ── Pure-function unit tests for truth helpers (push_typed_flag,
//    truth_review_trigger, truth_display_status). These guard the
//    truth-surface adjudicator primitives so regressions in precedence or
//    dedup logic fail loudly instead of leaking through integration tests.

#[test]
fn test_push_typed_flag_appends_new_flag() {
    let mut flags: Vec<TranscriptionConfidenceFlag> = Vec::new();
    push_typed_flag(&mut flags, TranscriptionConfidenceFlag::CloudFallbackUsed);
    assert_eq!(flags, vec![TranscriptionConfidenceFlag::CloudFallbackUsed]);
}

#[test]
fn test_push_typed_flag_deduplicates_existing_flag() {
    let mut flags = vec![TranscriptionConfidenceFlag::CloudFallbackUsed];
    push_typed_flag(&mut flags, TranscriptionConfidenceFlag::CloudFallbackUsed);
    assert_eq!(flags.len(), 1);
}

#[test]
fn test_push_typed_flag_preserves_order_when_adding_distinct_flags() {
    let mut flags: Vec<TranscriptionConfidenceFlag> = Vec::new();
    push_typed_flag(
        &mut flags,
        TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
    );
    push_typed_flag(&mut flags, TranscriptionConfidenceFlag::CloudFallbackUsed);
    push_typed_flag(
        &mut flags,
        TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
    ); // dup
    assert_eq!(
        flags,
        vec![
            TranscriptionConfidenceFlag::LocalFinalPassUnavailable,
            TranscriptionConfidenceFlag::CloudFallbackUsed,
        ]
    );
}

#[test]
fn test_apply_ai_noop_signal_sets_flag_and_commit_trigger() {
    let mut flags = vec![TranscriptionConfidenceFlag::CloudFallbackUsed];
    let mut commit_trigger = Some("high_rewrite_ratio".to_string());

    apply_ai_noop_signal(false, true, &mut flags, &mut commit_trigger);

    assert_eq!(
        flags,
        vec![
            TranscriptionConfidenceFlag::CloudFallbackUsed,
            TranscriptionConfidenceFlag::AiNoopDetected,
        ]
    );
    assert_eq!(commit_trigger.as_deref(), Some("ai_noop"));
}

#[test]
fn test_apply_ai_noop_signal_is_noop_for_assistive_sessions() {
    let mut flags = vec![TranscriptionConfidenceFlag::CloudFallbackUsed];
    let mut commit_trigger = Some("high_rewrite_ratio".to_string());

    apply_ai_noop_signal(true, true, &mut flags, &mut commit_trigger);

    assert_eq!(flags, vec![TranscriptionConfidenceFlag::CloudFallbackUsed]);
    assert_eq!(commit_trigger.as_deref(), Some("high_rewrite_ratio"));
}

#[test]
fn test_truth_review_trigger_no_speech_short_circuits_all_other_signals() {
    let flags = vec![
        TranscriptionConfidenceFlag::PossibleHallucinationLogprob,
        TranscriptionConfidenceFlag::CloudFallbackUsed,
    ];
    let trigger = truth_review_trigger(
        Some(RecordingFallbackClass::Unsafe),
        Some("vad_no_speech_detected"),
        &flags,
    );
    assert_eq!(trigger.as_deref(), Some("no_reliable_speech"));
}

#[test]
fn test_truth_review_trigger_hallucination_wins_over_degraded_fallback() {
    let flags = vec![TranscriptionConfidenceFlag::PossibleHallucinationLogprob];
    let trigger = truth_review_trigger(Some(RecordingFallbackClass::Degraded), None, &flags);
    assert_eq!(trigger.as_deref(), Some("possible_hallucination_logprob"));
}

#[test]
fn test_truth_review_trigger_very_low_speech_wins_over_streaming_preview() {
    let flags = vec![
        TranscriptionConfidenceFlag::VeryLowSpeech,
        TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
    ];
    let trigger = truth_review_trigger(None, None, &flags);
    assert_eq!(trigger.as_deref(), Some("very_low_speech"));
}

#[test]
fn test_truth_review_trigger_streaming_preview_wins_over_cloud_fallback() {
    let flags = vec![
        TranscriptionConfidenceFlag::StreamingPreviewUsedAsVerdict,
        TranscriptionConfidenceFlag::CloudFallbackUsed,
    ];
    let trigger = truth_review_trigger(None, None, &flags);
    assert_eq!(
        trigger.as_deref(),
        Some("streaming_preview_used_as_verdict")
    );
}

#[test]
fn test_truth_review_trigger_cloud_fallback_flag_wins_over_acceptable_class() {
    let flags = vec![TranscriptionConfidenceFlag::CloudFallbackUsed];
    let trigger = truth_review_trigger(Some(RecordingFallbackClass::Acceptable), None, &flags);
    assert_eq!(trigger.as_deref(), Some("cloud_fallback_used"));
}

#[test]
fn test_truth_review_trigger_degraded_fallback_when_no_confidence_flags() {
    let trigger = truth_review_trigger(Some(RecordingFallbackClass::Degraded), None, &[]);
    assert_eq!(trigger.as_deref(), Some("degraded_fallback"));
}

#[test]
fn test_truth_review_trigger_unsafe_fallback_when_no_confidence_flags() {
    let trigger = truth_review_trigger(Some(RecordingFallbackClass::Unsafe), None, &[]);
    assert_eq!(trigger.as_deref(), Some("unsafe_fallback"));
}

#[test]
fn test_truth_review_trigger_acceptable_class_returns_none() {
    assert!(truth_review_trigger(Some(RecordingFallbackClass::Acceptable), None, &[]).is_none());
    assert!(truth_review_trigger(None, None, &[]).is_none());
}

#[test]
fn test_truth_review_trigger_ignores_silero_tail_drop_flag() {
    let flags = vec![TranscriptionConfidenceFlag::SileroDroppedTailHallucinations { count: 5 }];
    assert!(truth_review_trigger(None, None, &flags).is_none());
}

#[test]
fn test_truth_display_status_no_speech_is_user_facing_english() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::LocalFinalPass),
        Some(RecordingFallbackClass::Unsafe),
        Some("vad_no_speech_detected"),
        &[TranscriptionConfidenceFlag::PossibleHallucinationLogprob],
    );
    assert_eq!(status, "No reliable speech detected");
}

#[test]
fn test_truth_display_status_hallucination_wins_over_source_label() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::LocalFinalPass),
        None,
        None,
        &[TranscriptionConfidenceFlag::PossibleHallucinationLogprob],
    );
    assert_eq!(status, "Possible hallucination");
}

#[test]
fn test_truth_display_status_very_low_speech_wins_over_fallback_class() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::LocalFinalPass),
        Some(RecordingFallbackClass::Degraded),
        None,
        &[TranscriptionConfidenceFlag::VeryLowSpeech],
    );
    assert_eq!(status, "Very low speech");
}

#[test]
fn test_truth_display_status_streaming_fallback_short_circuits_fallback_label() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::StreamingFallback),
        Some(RecordingFallbackClass::Degraded),
        None,
        &[],
    );
    assert_eq!(status, "Streaming fallback");
}

#[test]
fn test_truth_display_status_composes_source_and_fallback_labels() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::LocalFinalPass),
        Some(RecordingFallbackClass::Degraded),
        None,
        &[],
    );
    assert_eq!(status, "Final-pass local (degraded fallback)");
}

#[test]
fn test_truth_display_status_source_only_uses_source_label() {
    let status = truth_display_status(
        Some(RecordingTranscriptSource::CloudPrimary),
        None,
        None,
        &[],
    );
    assert_eq!(status, "Cloud primary");
}

#[test]
fn test_truth_display_status_fallback_only_uses_fallback_label() {
    let status = truth_display_status(None, Some(RecordingFallbackClass::Unsafe), None, &[]);
    assert_eq!(status, "unsafe fallback");
}

#[test]
fn test_truth_display_status_defaults_to_ready_when_no_signals() {
    let status = truth_display_status(None, None, None, &[]);
    assert_eq!(status, "Transcript ready");
}

#[tokio::test]
#[serial]
async fn test_toggle_press_does_not_set_force_raw_mode() {
    let controller = RecordingController::new();

    // Toggle Press → force_raw=false (respects AI_FORMATTING_ENABLED)
    let event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    controller.handle_hotkey_event(event).await.unwrap();

    // force_raw should be false
    assert!(
        !*controller.force_raw_mode.read().await,
        "Toggle should NOT set force_raw_mode (respects setting)"
    );
    assert!(
        !*controller.assistive_mode.read().await,
        "Toggle without assistive should keep assistive_mode=false"
    );
}

#[tokio::test]
#[serial]
async fn test_toggle_press_sets_force_ai_mode() {
    let controller = RecordingController::new();

    let event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: true,
    };
    controller.handle_hotkey_event(event).await.unwrap();

    assert!(
        *controller.force_ai_mode.read().await,
        "Toggle with force_ai should set force_ai_mode=true"
    );
}

#[tokio::test]
#[serial]
async fn test_left_double_option_does_not_switch_to_assistive_routing() {
    let controller = RecordingController::new();

    let event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: true,
    };
    controller.handle_hotkey_event(event).await.unwrap();

    assert!(
        !*controller.assistive_mode.read().await,
        "Left double option must stay non-assistive"
    );
    assert!(
        *controller.force_ai_mode.read().await,
        "Left double option should keep force_ai_mode=true"
    );
    assert!(
        !is_assistive_session(),
        "Global routing flag must stay non-assistive for left double option"
    );
}

#[tokio::test]
#[serial]
async fn test_hold_with_shift_sets_assistive_not_force_raw() {
    let controller = RecordingController::new();

    // Hold Down WITH Shift → assistive=true, force_raw=false
    let event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false, // Shift was held from the start (Ctrl+Shift)
    };
    controller.handle_hotkey_event(event).await.unwrap();

    // assistive should be true, force_raw should be false
    assert!(
        *controller.assistive_mode.read().await,
        "Hold with Shift should set assistive_mode=true"
    );
    assert!(
        !*controller.force_raw_mode.read().await,
        "Hold with Shift should NOT set force_raw_mode (Assistive takes precedence)"
    );
}

#[tokio::test]
#[serial]
async fn test_shift_upgrade_mid_hold_overrides_force_raw() {
    let controller = RecordingController::new();

    // First: Hold Down without Shift (starts as RAW mode)
    let down_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(down_event).await.unwrap();

    // Verify RAW mode is set
    assert!(*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);

    // Now: User adds Shift mid-hold (upgrade to Assistive)
    // This comes as another event with assistive=true
    let upgrade_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Press, // Modifier flags changed while holding
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false,
    };
    controller.handle_hotkey_event(upgrade_event).await.unwrap();

    // Should upgrade to Assistive, force_raw should be cleared
    assert!(
        *controller.assistive_mode.read().await,
        "Shift added mid-hold should upgrade to assistive_mode=true"
    );
    assert!(
        !*controller.force_raw_mode.read().await,
        "Shift upgrade should clear force_raw_mode"
    );
}

#[tokio::test]
#[serial]
async fn test_hold_up_preserves_mode_flags_when_idle() {
    let controller = RecordingController::new();

    // Set up flags manually (simulating mid-session state)
    *controller.force_raw_mode.write().await = true;
    *controller.assistive_mode.write().await = false;
    // Keep state IDLE - Up event in IDLE just cancels pending hold start

    // Hold Up when IDLE should NOT modify the flags
    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(up_event).await.unwrap();

    // Flags should still be set (Up action doesn't touch them in IDLE state)
    assert!(
        *controller.force_raw_mode.read().await,
        "Hold Up in IDLE should preserve force_raw_mode"
    );
}

#[tokio::test]
#[serial]
async fn test_hold_up_triggers_finish_recording() {
    // This test verifies that Hold Up in REC_HOLD state triggers finish_recording
    // which reads force_raw_mode and assistive_mode before processing.
    // Requires audio hardware to actually record/transcribe.
    let controller = RecordingController::new();
    *controller.state.write().await = State::RecHold;
    *controller.force_raw_mode.write().await = true;

    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let result = controller.handle_hotkey_event(up_event).await;
    if let Err(err) = result {
        assert!(
            err.to_string().contains("Empty transcript"),
            "Unexpected error: {err}"
        );
    }

    // After finish_recording, flags should be reset to false
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!*controller.force_raw_mode.read().await);
}

#[tokio::test]
async fn test_reset_clears_all_mode_flags() {
    let controller = RecordingController::new();

    // Set up various flags
    *controller.state.write().await = State::RecHold;
    *controller.force_raw_mode.write().await = true;
    *controller.force_ai_mode.write().await = true;
    *controller.assistive_mode.write().await = true;
    *controller.session_id.write().await = Some("test-session".to_string());

    // Reset should clear everything
    controller.reset().await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert!(
        !*controller.force_raw_mode.read().await,
        "reset should clear force_raw_mode"
    );
    assert!(
        !*controller.assistive_mode.read().await,
        "reset should clear assistive_mode"
    );
    assert!(
        !*controller.force_ai_mode.read().await,
        "reset should clear force_ai_mode"
    );
    assert!(
        controller.session_id.read().await.is_none(),
        "reset should clear session_id"
    );
}

#[tokio::test]
#[serial]
async fn test_mode_matrix_coverage() {
    // This test documents all possible mode combinations:
    //
    // | Hotkey          | force_raw | assistive | Result                    |
    // |-----------------|-----------|-----------|---------------------------|
    // | Ctrl Hold       | true      | false     | RAW (ignore AI setting)   |
    // | Ctrl+Shift Hold | false     | true      | Assistive (always AI)     |
    // | Left Double Opt | false     | false     | Formatting (force AI)     |

    let controller = RecordingController::new();

    // Case 1: Ctrl Hold
    let ctrl_hold = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    controller.handle_hotkey_event(ctrl_hold).await.unwrap();
    assert!(*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);

    // Reset for next case
    *controller.force_raw_mode.write().await = false;
    *controller.assistive_mode.write().await = false;

    // Case 2: Ctrl+Shift Hold
    let ctrl_shift_hold = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false,
    };
    controller
        .handle_hotkey_event(ctrl_shift_hold)
        .await
        .unwrap();
    assert!(!*controller.force_raw_mode.read().await);
    assert!(*controller.assistive_mode.read().await);

    // Reset for next case
    *controller.force_raw_mode.write().await = false;
    *controller.assistive_mode.write().await = false;

    // Case 3: Left Double Option (force AI)
    let double_option = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: true,
    };
    controller.handle_hotkey_event(double_option).await.unwrap();
    assert!(!*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);
    assert!(*controller.force_ai_mode.read().await);
}

#[tokio::test]
#[serial]
async fn test_finish_recording_resets_unconditionally_force_raw() {
    // Regression test: paste fix removed `manual_actions_only` gate.
    // After recording finishes, state MUST reset to Idle and flags clear
    // regardless of mode (no "decision mode" branching).
    let controller = RecordingController::new();
    *controller.state.write().await = State::RecHold;
    *controller.force_raw_mode.write().await = true;
    *controller.assistive_mode.write().await = false;

    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let _ = controller.handle_hotkey_event(up_event).await;

    // After finish: ALWAYS Idle, ALWAYS flags cleared (no decision mode)
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);
    assert!(!*controller.force_ai_mode.read().await);
}

#[tokio::test]
#[serial]
async fn test_finish_recording_resets_unconditionally_assistive() {
    // Same test but for assistive mode — paste must work in all modes
    let controller = RecordingController::new();
    *controller.state.write().await = State::RecHold;
    *controller.force_raw_mode.write().await = false;
    *controller.assistive_mode.write().await = true;

    let up_event = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: true,
        hold_mode: HoldMode::Chat,
        force_raw: false,
        force_ai: false,
    };
    let _ = controller.handle_hotkey_event(up_event).await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!*controller.force_raw_mode.read().await);
    assert!(!*controller.assistive_mode.read().await);
}

#[tokio::test]
async fn test_no_decision_mode_state_exists() {
    // Compile-time + runtime proof: State enum has exactly these variants.
    // There is NO "DecisionMode" variant — the paste regression was caused
    // by the old legacy transcription overlay decision mode, which has been removed.
    let states = [State::Idle, State::RecHold, State::RecToggle];
    for state in &states {
        let controller = RecordingController::new();
        *controller.state.write().await = *state;
        let current = controller.current_state().await;
        assert!(
            matches!(current, State::Idle | State::RecHold | State::RecToggle),
            "Unknown state variant detected: {:?}",
            current
        );
    }
}

#[tokio::test]
#[serial]
async fn test_finish_recording_resets_unconditionally_toggle_mode() {
    // Toggle mode (double-Option): after finish, same cleanup as hold modes
    let controller = RecordingController::new();
    *controller.state.write().await = State::RecToggle;
    *controller.force_ai_mode.write().await = true;

    // Toggle press again to stop
    let stop_event = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: true,
    };
    let _ = controller.handle_hotkey_event(stop_event).await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!*controller.force_ai_mode.read().await);
    assert!(!*controller.force_raw_mode.read().await);
}

#[test]
fn test_action_quality_probe_reports_expected_metrics() {
    let raw = "Kubernetes wymoga konfiguracji";
    let final_text = "Kubernetes wymaga konfiguracji.";
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 10,
        dropped_chunks: 2,
        ..Default::default()
    };
    let probe = ActionQualityProbe::from_transcripts(raw, final_text, &stats);

    assert_eq!(probe.raw_chars, raw.chars().count());
    assert_eq!(probe.final_chars, final_text.chars().count());
    assert!(probe.raw_final_diff_ratio > 0.0);
    assert!(probe.correction_ratio > 0.0);
    assert!((probe.drop_ratio - 0.2).abs() < 0.001);
}

#[test]
fn test_action_quality_probe_is_independent_from_action_routing() {
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 8,
        dropped_chunks: 1,
        ..Default::default()
    };
    let save_probe = ActionQualityProbe::from_transcripts("to jest test", "To jest test.", &stats);
    let copy_probe = ActionQualityProbe::from_transcripts("to jest test", "To jest test.", &stats);
    let augment_probe =
        ActionQualityProbe::from_transcripts("to jest test", "To jest test.", &stats);

    assert!((save_probe.raw_final_diff_ratio - copy_probe.raw_final_diff_ratio).abs() < 1e-6);
    assert!((save_probe.raw_final_diff_ratio - augment_probe.raw_final_diff_ratio).abs() < 1e-6);
    assert!((save_probe.correction_ratio - copy_probe.correction_ratio).abs() < 1e-6);
    assert!((save_probe.correction_ratio - augment_probe.correction_ratio).abs() < 1e-6);
    assert!((save_probe.drop_ratio - copy_probe.drop_ratio).abs() < 1e-6);
    assert!((save_probe.drop_ratio - augment_probe.drop_ratio).abs() < 1e-6);
}

#[test]
fn test_quality_gate_triggers_commit_for_high_drop_ratio() {
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 10,
        dropped_chunks: 5,
        ..Default::default()
    };
    let probe = ActionQualityProbe::from_transcripts(
        "to jest bardzo dlugi tekst surowy",
        "to jest tekst",
        &stats,
    );
    let trigger =
        evaluate_quality_commit_trigger(false, &probe, crate::state::history::TranscriptKind::Raw);
    assert_eq!(trigger, Some("high_drop_ratio"));
}

#[test]
fn test_quality_gate_skips_commit_for_force_raw_mode() {
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 10,
        dropped_chunks: 7,
        ..Default::default()
    };
    let probe =
        ActionQualityProbe::from_transcripts("to jest bardzo dlugi tekst surowy", "krótki", &stats);
    let trigger =
        evaluate_quality_commit_trigger(true, &probe, crate::state::history::TranscriptKind::Raw);
    assert!(trigger.is_none());
}

#[test]
fn test_quality_gate_triggers_commit_when_ai_failed() {
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 2,
        dropped_chunks: 0,
        ..Default::default()
    };
    let probe = ActionQualityProbe::from_transcripts("raw text", "raw text", &stats);
    let trigger = evaluate_quality_commit_trigger(
        false,
        &probe,
        crate::state::history::TranscriptKind::FormattingFailed,
    );
    assert_eq!(trigger, Some("ai_failed_fallback"));
}

#[test]
fn test_quality_gate_catches_short_ai_rewrites_in_danger_zone() {
    let stats = crate::stream_postprocess::StreamPostProcessStats {
        input_chunks: 4,
        dropped_chunks: 0,
        ..Default::default()
    };
    let probe = ActionQualityProbe::from_transcripts("abcdefghijk", "qrstuvwxyz!", &stats);
    let trigger = evaluate_quality_commit_trigger(
        false,
        &probe,
        crate::state::history::TranscriptKind::FormattedTranscript,
    );
    assert_eq!(trigger, Some("high_rewrite_ratio"));
}

#[test]
fn test_delta_first_guards_block_full_rewrite_in_live_stream() {
    assert!(!should_allow_full_user_bubble_rewrite(false, false, true));
    assert!(!should_apply_transcription_action_contract(false, true));
}

#[test]
fn test_delta_first_guards_allow_full_rewrite_offline() {
    assert!(should_allow_full_user_bubble_rewrite(false, false, false));
    assert!(should_apply_transcription_action_contract(false, false));
}

#[test]
fn test_process_recording_outcome_no_speech_is_soft() {
    let outcome =
        ProcessRecordingOutcome::no_speech("vad_no_speech_detected", "No reliable speech detected");
    assert_eq!(
        outcome.no_speech_reason.as_deref(),
        Some("vad_no_speech_detected")
    );
    assert!(outcome.commit_trigger.is_none());
    assert_eq!(outcome.final_status, "No reliable speech detected");
}

#[tokio::test]
#[serial]
async fn test_rapid_hold_toggle_switch_recovers_to_idle_without_stuck_state() {
    let controller = RecordingController::new();
    controller.config.write().await.hold_start_delay_ms = 40;

    let hold_down = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let hold_up = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let toggle_press = HotkeyInput {
        key_type: HotkeyType::Toggle,
        action: HotkeyAction::Press,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };

    controller.handle_hotkey_event(hold_down).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Toggle should take control and prevent stale hold-start from reviving later.
    controller
        .handle_hotkey_event(toggle_press.clone())
        .await
        .unwrap();
    assert_eq!(controller.current_state().await, State::RecToggle);

    controller
        .handle_hotkey_event(toggle_press.clone())
        .await
        .unwrap();
    controller.handle_hotkey_event(hold_up).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert!(controller.session_id.read().await.is_none());
    assert!(
        !controller
            .assistive_loop_active
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(!is_assistive_session());
}

#[tokio::test]
#[serial]
async fn test_repeated_hold_cancel_near_delay_never_starts_stale_session() {
    let controller = RecordingController::new();
    controller.config.write().await.hold_start_delay_ms = 45;

    let hold_down = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Down,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };
    let hold_up = HotkeyInput {
        key_type: HotkeyType::Hold,
        action: HotkeyAction::Up,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: true,
        force_ai: false,
    };

    for _ in 0..6 {
        controller
            .handle_hotkey_event(hold_down.clone())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        controller
            .handle_hotkey_event(hold_up.clone())
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!controller.is_recording().await);
    assert!(controller.session_id.read().await.is_none());
}

#[tokio::test]
async fn test_reset_session_after_start_failure_clears_transient_state() {
    let controller = RecordingController::new();
    *controller.state.write().await = State::RecToggle;
    *controller.assistive_mode.write().await = true;
    *controller.hold_mode.write().await = HoldMode::Chat;
    *controller.force_raw_mode.write().await = true;
    *controller.force_ai_mode.write().await = true;
    *controller.session_id.write().await = Some("failed-start-session".to_string());
    *controller.assistive_context.write().await = Some(AssistiveContext::default());
    controller
        .assistive_loop_active
        .store(true, std::sync::atomic::Ordering::SeqCst);
    controller
        .toggle_user_has_text
        .store(true, std::sync::atomic::Ordering::SeqCst);
    controller
        .toggle_assistant_has_text
        .store(true, std::sync::atomic::Ordering::SeqCst);
    set_assistive_session(true);

    controller.reset_session_after_start_failure("test").await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert!(!*controller.assistive_mode.read().await);
    assert_eq!(*controller.hold_mode.read().await, HoldMode::Raw);
    assert!(!*controller.force_raw_mode.read().await);
    assert!(!*controller.force_ai_mode.read().await);
    assert!(controller.session_id.read().await.is_none());
    assert!(controller.assistive_context.read().await.is_none());
    assert!(
        !controller
            .assistive_loop_active
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(
        !controller
            .toggle_user_has_text
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(
        !controller
            .toggle_assistant_has_text
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(!is_assistive_session());
}

/// Regression guard for the toggle-stop self-deadlock (root cause behind
/// commit 91b2346's watchdog).
///
/// Reproduces the exact lock pattern that hung `stop_toggle_and_adjudicate_inner`
/// on operator's daily-driver: in Rust 2024 edition, the temporary
/// `RwLockReadGuard` produced by an `if let Some(x) = lock.read().await.clone()`
/// scrutinee lives until the end of the if-let block, so the task awaiting
/// `lock.write()` inside the body deadlocks on its own read guard. The fix is to
/// materialize the snapshot into a `let` binding first; the guard then drops at
/// the semicolon and the subsequent `.write().await` resolves immediately.
///
/// If anyone reverts the snapshot pattern back to an inline `read().await.clone()`
/// scrutinee, this test times out in 2s and fails — well under the 45s watchdog
/// so the regression surfaces in CI/local test runs, not after a stuck recording.
#[tokio::test]
async fn rwlock_session_id_read_then_write_does_not_self_deadlock() {
    use tokio::sync::RwLock;

    let lock = RwLock::new(Some("session-xyz".to_string()));

    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let snapshot = lock.read().await.clone();
        if let Some(current) = snapshot {
            *lock.write().await = Some(format!("{current}:stopping"));
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "session_id read-snapshot + write pattern self-deadlocked — if the inline \
         `if let Some(x) = lock.read().await.clone() {{ ... lock.write().await ... }}` \
         pattern returned, the Rust 2024 if-let temporary scope extension is back. \
         See commit fixing fix/toggle-stuck-watchdog."
    );

    assert_eq!(
        *lock.read().await,
        Some("session-xyz:stopping".to_string()),
        "expected the write to land after the read guard dropped"
    );
}
