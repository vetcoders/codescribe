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
        crate::transcription_overlay::TranscriptionActionContractMode::Raw
    );
}

#[test]
fn test_action_contract_mode_uses_ai_format_when_force_ai_enabled() {
    let mode = resolve_transcription_action_contract_mode(false, true, false, false);
    assert_eq!(
        mode,
        crate::transcription_overlay::TranscriptionActionContractMode::AiFormat
    );
}

#[test]
fn test_action_contract_mode_uses_ai_format_for_toggle_ai_path() {
    let mode = resolve_transcription_action_contract_mode(false, false, true, true);
    assert_eq!(
        mode,
        crate::transcription_overlay::TranscriptionActionContractMode::AiFormat
    );
}

#[test]
fn test_action_contract_mode_uses_raw_for_toggle_without_ai() {
    let mode = resolve_transcription_action_contract_mode(false, false, true, false);
    assert_eq!(
        mode,
        crate::transcription_overlay::TranscriptionActionContractMode::Raw
    );
}

#[test]
fn test_select_recording_transcript_prefers_local_final_pass_for_local_backend() {
    let (raw_text, cloud_text, source) = select_recording_transcript(
        true,
        Some("local final".to_string()),
        "streaming fallback".to_string(),
        None,
    );

    assert_eq!(raw_text.as_deref(), Some("local final"));
    assert_eq!(cloud_text, None);
    assert_eq!(source, Some(RecordingTranscriptSource::LocalFinalPass));
}

#[test]
fn test_select_recording_transcript_prefers_cloud_for_cloud_backend() {
    let (raw_text, cloud_text, source) = select_recording_transcript(
        false,
        None,
        "streaming fallback".to_string(),
        Some("cloud final".to_string()),
    );

    assert_eq!(raw_text.as_deref(), Some("cloud final"));
    assert_eq!(cloud_text.as_deref(), Some("cloud final"));
    assert_eq!(source, Some(RecordingTranscriptSource::CloudPrimary));
}

#[test]
fn test_select_recording_transcript_falls_back_to_streaming_when_cloud_missing() {
    let (raw_text, cloud_text, source) =
        select_recording_transcript(false, None, "streaming fallback".to_string(), None);

    assert_eq!(raw_text.as_deref(), Some("streaming fallback"));
    assert_eq!(cloud_text, None);
    assert_eq!(source, Some(RecordingTranscriptSource::StreamingFallback));
}

#[test]
fn test_select_recording_transcript_ignores_empty_candidates() {
    let (raw_text, cloud_text, source) = select_recording_transcript(
        false,
        Some("   ".to_string()),
        "  ".to_string(),
        Some("".to_string()),
    );

    assert_eq!(raw_text, None);
    assert_eq!(cloud_text, None);
    assert_eq!(source, None);
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

#[tokio::test]
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
    // by `enter_decision_mode()` which has been replaced by `schedule_auto_hide()`.
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
        crate::state::history::TranscriptKind::AiFailed,
    );
    assert_eq!(trigger, Some("ai_failed_fallback"));
}

#[test]
fn test_delta_first_guards_block_full_rewrite_in_live_stream() {
    assert!(!should_allow_full_user_bubble_rewrite(false, false, true));
    assert!(!should_allow_full_assistant_rewrite(false, true));
    assert!(!should_apply_transcription_action_contract(false, true));
}

#[test]
fn test_delta_first_guards_allow_full_rewrite_offline() {
    assert!(should_allow_full_user_bubble_rewrite(false, false, false));
    assert!(should_allow_full_assistant_rewrite(false, false));
    assert!(should_apply_transcription_action_contract(false, false));
}

#[test]
fn test_process_recording_outcome_no_speech_is_soft() {
    let outcome = ProcessRecordingOutcome::no_speech("vad_no_speech_detected");
    assert_eq!(
        outcome.no_speech_reason.as_deref(),
        Some("vad_no_speech_detected")
    );
    assert!(outcome.commit_trigger.is_none());
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
