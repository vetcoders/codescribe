use super::*;
use crate::os::tray_status::{self, TrayStatus, TrayStatusSnapshot};

struct TrayRestore(TrayStatusSnapshot, bool);

impl TrayRestore {
    fn capture() -> Self {
        Self(
            tray_status::current_tray_status_snapshot(),
            is_assistive_session(),
        )
    }
}

impl Drop for TrayRestore {
    fn drop(&mut self) {
        set_assistive_session(self.1);
        tray_status::update_tray_status(self.0.status);
        tray_status::set_tray_indicator_mode(self.0.indicator_mode);
    }
}

/// Silver: refusal never changes controller Idle, so waiting for a later
/// StateChange cannot clear the cursor badge or tray Error. Exercise the real
/// start path with the instance admission seam and read its real IPC projection.
#[tokio::test]
#[serial_test::serial]
async fn refused_toggle_start_settles_badge_and_tray_while_controller_stays_idle() {
    let _restore = TrayRestore::capture();
    for assistive in [false, true] {
        for blocker in [
            admission::AdmissionBlocker::MicrophonePermissionUnavailable {
                status: "denied".into(),
            },
            admission::AdmissionBlocker::CaptureDeviceUnavailable {
                reason: "fixture input device is absent".into(),
            },
        ] {
            let controller = RecordingController::new_without_keychain();
            let mut events = controller.event_broadcast.subscribe();
            controller.install_test_start_admission(Err(blocker.clone()));
            tray_status::update_tray_status(TrayStatus::Idle);
            let badge_generation = crate::os::hold_badge::take_token();

            let result = controller
                .start_toggle_recording(assistive, CaptureTurnIntent::HandsFree)
                .await;

            assert!(result.is_err(), "the physical capture is refused");
            assert_eq!(controller.current_state().await, State::Idle);
            assert!(controller.session_id.read().await.is_none());
            assert!(!controller.start_transition_in_flight.load(Ordering::SeqCst));
            assert!(!controller.assistive_loop_active.load(Ordering::SeqCst));
            assert_eq!(tray_status::current_tray_status(), TrayStatus::Idle);
            #[cfg(target_os = "macos")]
            assert!(
                crate::os::hold_badge::take_token() > badge_generation,
                "the canonical badge generation must retire any queued show"
            );
            #[cfg(not(target_os = "macos"))]
            let _ = badge_generation;

            let mut statuses = Vec::new();
            while let Ok(event) = events.try_recv() {
                if let IpcEventPayload::PresentationStatus { json } = event.payload {
                    statuses.push(
                        serde_json::from_str::<PresentationStatusProjection>(&json)
                            .expect("valid typed refusal projection"),
                    );
                }
            }
            assert_eq!(statuses.len(), 1, "one explanation, no fake transcript");
            let status = &statuses[0];
            assert_eq!(status.code, blocker.code());
            assert!(
                status
                    .session_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
            );
            assert!(status.message.contains(&blocker.explanation()));
            assert!(status.message.contains(&blocker.action()));
            assert!(status.terminal);
        }
    }
}

/// A recorder/preflight failure uses this same cleanup after admission. It
/// must invalidate pending native badge shows without relying on the bridge.
#[tokio::test]
#[serial_test::serial]
async fn failed_start_cleanup_retires_badge_generation_and_stuck_error() {
    let _restore = TrayRestore::capture();
    let controller = RecordingController::new_without_keychain();
    tray_status::update_tray_status(TrayStatus::Error);
    let badge_generation = crate::os::hold_badge::take_token();

    controller
        .reset_session_after_start_failure("Silver preflight failure")
        .await;

    assert_eq!(controller.current_state().await, State::Idle);
    assert_eq!(tray_status::current_tray_status(), TrayStatus::Idle);
    #[cfg(target_os = "macos")]
    assert!(crate::os::hold_badge::take_token() > badge_generation);
    #[cfg(not(target_os = "macos"))]
    let _ = badge_generation;
}

/// A second start cannot settle chrome owned by an already active take.
#[tokio::test]
#[serial_test::serial]
async fn ignored_competing_start_preserves_active_recording_chrome() {
    let _restore = TrayRestore::capture();
    let controller = RecordingController::new_without_keychain();
    *controller.state.write().await = State::RecToggle;
    tray_status::update_tray_status(TrayStatus::Listening);
    tray_status::set_tray_indicator_mode(BadgeMode::Assistive);
    let before = tray_status::current_tray_status_snapshot();
    let badge_generation = crate::os::hold_badge::take_token();

    let result = controller
        .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
        .await
        .expect("a competing start is ignored");

    assert!(matches!(result, CaptureAdmission::NotAdmitted));
    assert_eq!(controller.current_state().await, State::RecToggle);
    assert_eq!(tray_status::current_tray_status_snapshot(), before);
    assert_eq!(crate::os::hold_badge::take_token(), badge_generation);
}

/// The controller owns capture. A stale tray projection cannot veto cleanup
/// after that owner has already settled a failed start back to Idle.
#[tokio::test]
#[serial_test::serial]
async fn failed_start_cleanup_does_not_mistake_stale_tray_paint_for_an_active_take() {
    let _restore = TrayRestore::capture();
    for status in [TrayStatus::Listening, TrayStatus::Thinking] {
        let controller = RecordingController::new_without_keychain();
        tray_status::update_tray_status(status);
        let badge_generation = crate::os::hold_badge::take_token();
        controller
            .reset_session_after_start_failure("stale tray projection")
            .await;
        assert_eq!(controller.current_state().await, State::Idle);
        assert_eq!(tray_status::current_tray_status(), TrayStatus::Idle);
        #[cfg(target_os = "macos")]
        assert!(crate::os::hold_badge::take_token() > badge_generation);
        #[cfg(not(target_os = "macos"))]
        let _ = badge_generation;
    }
}
