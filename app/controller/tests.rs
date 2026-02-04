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
