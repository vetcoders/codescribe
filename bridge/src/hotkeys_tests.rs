pub use super::*;

#[cfg(test)]
mod max_consultation_inspection_tests {
    use super::CsMaxConsultationSnapshot;
    use codescribe_core::agent::thread_delivery::{
        ConsultationInputSnapshot, ConsultationRecoverySnapshot,
    };
    use codescribe_core::agent::{ContentBlock, Message, Role};

    #[test]
    fn retained_projection_preserves_source_text_identity_and_attachment_count() {
        let snapshot = CsMaxConsultationSnapshot::from(ConsultationRecoverySnapshot {
            consultation_id: "selected-max".into(),
            pending_turn_id: Some("uncertain-turn".into()),
            retained_inputs: vec![ConsultationInputSnapshot {
                turn_id: "uncertain-turn".into(),
                provider_name: "formatting-provider".into(),
                input: Message::new(
                    Role::User,
                    vec![
                        ContentBlock::Text("Prepare git add; do not execute it.".into()),
                        ContentBlock::Image {
                            media_type: "image/png".into(),
                            data: b"fixture".to_vec(),
                        },
                        ContentBlock::Text("Keep the exact clipboard paths.".into()),
                    ],
                ),
            }],
        });
        assert_eq!(snapshot.consultation_id, "selected-max");
        assert_eq!(snapshot.pending_turn_id.as_deref(), Some("uncertain-turn"));
        assert_eq!(snapshot.retained_inputs.len(), 1);
        let input = &snapshot.retained_inputs[0];
        assert_eq!(input.turn_id, "uncertain-turn");
        assert_eq!(input.provider_name, "formatting-provider");
        assert_eq!(input.image_count, 1);
        assert_eq!(
            input.text_blocks,
            vec![
                "Prepare git add; do not execute it.",
                "Keep the exact clipboard paths.",
            ]
        );
    }
}

/// Dependency-mode fixture: app/core are normal dependencies, so cfg(test) in
/// those crates cannot protect this path. Keep the real controller and Stop state.
#[cfg(test)]
fn fixture_controller() -> Arc<RecordingController> {
    use codescribe::controller::ControllerStartupResources;
    use codescribe_core::config::{CapturedRuntimeInputs, Config, StartupAcquisitionProbe};
    let root = tempfile::tempdir().expect("isolated fixture root");
    let probe = StartupAcquisitionProbe::forbid();
    let snapshot = Config::runtime_snapshot_from_captured(CapturedRuntimeInputs::defaults_at(
        root.path().to_path_buf(),
        1_700_000_000_000,
    ));
    let controller = RecordingController::from_startup_inputs(
        snapshot,
        ControllerStartupResources::inert(),
        root.path(),
    );
    assert!(
        probe.attempts().is_empty(),
        "fixture acquired host startup inputs"
    );
    // Context storage is lazy. These lifecycle fixtures never write context;
    // the controller retains this explicit path after the scratch directory closes.
    Arc::new(controller)
}

#[cfg(test)]
mod application_shutdown_tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn shutdown_releases_controller_capture_ownership() {
        CAPTURE_OWNER.store(CAPTURE_OWNER_CONTROLLER, Ordering::SeqCst);
        release_capture_ownership_for_shutdown();
        assert_eq!(
            CAPTURE_OWNER.load(Ordering::SeqCst),
            CAPTURE_OWNER_NONE,
            "application shutdown must never leave microphone ownership latched"
        );
    }

    #[test]
    fn shutdown_requires_terminal_outcome_and_resource_quiescence() {
        for outcome in [
            CaptureStopOutcome::Pending,
            CaptureStopOutcome::AlreadyStopping,
            CaptureStopOutcome::AdmissionUnavailable,
            CaptureStopOutcome::ForeignCapture,
        ] {
            assert!(require_shutdown_settlement(Ok(outcome), false).is_err());
            assert!(require_shutdown_settlement(Ok(outcome), true).is_err());
        }
        for outcome in [
            CaptureStopOutcome::Stopped,
            CaptureStopOutcome::NoLiveCapture,
        ] {
            assert!(require_shutdown_settlement(Ok(outcome), false).is_err());
            assert!(require_shutdown_settlement(Ok(outcome), true).is_ok());
        }
        assert!(require_shutdown_settlement(Err(anyhow::anyhow!("archive failed")), true).is_err());
    }

    #[tokio::test]
    async fn idle_shutdown_helper_closes_same_controller_without_removing_it() {
        let controller = fixture_controller();
        let store = Arc::new(Mutex::new(Some(Arc::clone(&controller))));
        settle_controller_for_shutdown(&controller).await.unwrap();
        assert!(Arc::ptr_eq(
            &current_controller(&store).unwrap(),
            &controller
        ));
        assert!(controller.capture_shutdown_settled());
        assert!(controller.start_composer_turn_recording().await.is_err());
    }

    #[tokio::test]
    async fn shutdown_recording_wait_is_bounded() {
        let error = await_recording_shutdown(
            std::future::pending::<Result<(), CsError>>(),
            Duration::from_millis(5),
        )
        .await
        .expect_err("pending stop must time out");
        let CsError::Recording { msg } = error else {
            panic!("shutdown timeout must be a recording error");
        };
        assert!(msg.contains("timed out"), "{msg}");
    }
}

/// Recording-dispatch side effects — notably that a blocked double-tap stays
/// silent on the tray instead of publishing a conflict the detector already owns.
#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use codescribe::os::hotkeys::{DoubleTapBlockReason, DoubleTapGesture};

    #[test]
    #[serial_test::serial]
    fn empty_channel_warns_once_and_preserves_existing_tray_status() {
        for refusal in [
            ChannelOpenRefusal::DigitUnbound { digit: 2 },
            ChannelOpenRefusal::BindingMissing {
                path: "fixture.json".into(),
            },
        ] {
            for status in [
                TrayStatus::Idle,
                TrayStatus::Listening,
                TrayStatus::Thinking,
                TrayStatus::Error,
            ] {
                tray_status::update_tray_status(status);
                let before = tray_status::current_tray_status_snapshot();
                let error =
                    anyhow::Error::new(refusal.clone()).context("dispatching agent shortcut");
                let mut notices = Vec::new();
                report_hotkey_dispatch_error(&error, |title, body| {
                    notices.push((title.to_owned(), body.to_owned()));
                });
                assert_eq!(notices, vec![("Codescribe".to_owned(), error.to_string())]);
                assert_eq!(
                    tray_status::current_tray_status_snapshot(),
                    before,
                    "{refusal:?}"
                );
            }
        }
        tray_status::update_tray_status(TrayStatus::Idle);
    }

    #[test]
    #[serial_test::serial]
    fn dispatch_failures_still_publish_persistent_error() {
        for error in [
            anyhow::Error::new(ChannelOpenRefusal::BindingMalformed {
                path: "fixture.json".into(),
                detail: "invalid JSON".into(),
            }),
            anyhow::Error::new(ChannelOpenRefusal::InvalidDigit),
            anyhow::anyhow!("recorder unavailable"),
            // Identical text without the refusal type must not hide a real failure.
            anyhow::anyhow!(ChannelOpenRefusal::DigitUnbound { digit: 2 }.to_string()),
        ] {
            tray_status::update_tray_status(TrayStatus::Idle);
            let mut notices = Vec::new();
            report_hotkey_dispatch_error(&error, |title, body| {
                notices.push((title.to_owned(), body.to_owned()));
            });
            assert_eq!(notices, vec![("Codescribe".to_owned(), error.to_string())]);
            assert_eq!(tray_status::current_tray_status(), TrayStatus::Error);
        }
        tray_status::update_tray_status(TrayStatus::Idle);
    }

    /// Detector already owns blocked-double-tap tray truth; bridge stays silent.
    #[tokio::test]
    #[serial_test::serial]
    async fn blocked_double_tap_does_not_publish_tray_conflict() {
        tray_status::update_tray_status(TrayStatus::Idle);

        let controller = fixture_controller();
        dispatch_recording_hotkey_event(
            HotkeyEvent::DoubleTapBlocked {
                gesture: DoubleTapGesture::LeftOption,
                reason: DoubleTapBlockReason::ModifierComboActive,
            },
            controller,
        )
        .await
        .expect("blocked double-tap dispatch should not fail");

        assert_eq!(tray_status::current_tray_status(), TrayStatus::Idle);
    }
}

/// The routing contract of [`route_hotkey_event`]: all capture modes reach one
/// recording callback; Agent UI notification carries no audio/transcript data.
#[cfg(test)]
mod app_action_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Test double that counts UI-only Agent summons.
    struct CountingAppActionListener {
        show_agent_calls: AtomicUsize,
        toggle_widget_calls: AtomicUsize,
        agent_gestures: Mutex<Vec<CsAgentGestureInput>>,
        composer_redirects: AtomicUsize,
    }

    impl CountingAppActionListener {
        fn new() -> Self {
            Self {
                show_agent_calls: AtomicUsize::new(0),
                toggle_widget_calls: AtomicUsize::new(0),
                agent_gestures: Mutex::new(Vec::new()),
                composer_redirects: AtomicUsize::new(0),
            }
        }
    }

    impl CsAppActionListener for CountingAppActionListener {
        fn on_max_approvals_changed(&self) {}
        /// Count ShowAgent UI callbacks (no recording side effects).
        fn on_show_agent(&self) {
            self.show_agent_calls.fetch_add(1, Ordering::SeqCst);
        }
        fn on_toggle_widget(&self) {
            self.toggle_widget_calls.fetch_add(1, Ordering::SeqCst);
        }
        fn on_agent_gesture(&self, input: CsAgentGestureInput) {
            self.agent_gestures
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(input);
        }
        fn on_composer_dictation_redirected(&self) {
            self.composer_redirects.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A refused hold reaches only the UI-only listener, exactly once.
    #[test]
    fn composer_dictation_redirect_is_ui_only() {
        let listener = Arc::new(CountingAppActionListener::new());
        notify_composer_dictation_redirected(Some(listener.clone()));
        notify_composer_dictation_redirected(None);
        assert_eq!(listener.composer_redirects.load(Ordering::SeqCst), 1);
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert_eq!(listener.toggle_widget_calls.load(Ordering::SeqCst), 0);
    }

    /// The chooser's input reaches the UI listener and never the recording path.
    #[test]
    fn agent_chooser_input_is_ui_only() {
        let listener = Arc::new(CountingAppActionListener::new());
        for input in [
            AgentGestureInput::MiddleClick,
            AgentGestureInput::Scroll { steps: -2 },
            AgentGestureInput::LeftClick,
            AgentGestureInput::Cancel,
        ] {
            route_hotkey_event(
                HotkeyEvent::AgentGesture(input),
                Some(listener.clone()),
                |event| panic!("chooser input reached the recording path: {event:?}"),
                || panic!("chooser input must not dispatch deferred insert"),
            );
        }
        assert_eq!(
            *listener
                .agent_gestures
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            vec![
                CsAgentGestureInput::MiddleClick,
                CsAgentGestureInput::Scroll { steps: -2 },
                CsAgentGestureInput::LeftClick,
                CsAgentGestureInput::Cancel,
            ]
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert!(!event_can_start_capture(&HotkeyEvent::AgentGesture(
            AgentGestureInput::MiddleClick
        )));
    }

    /// Every session-starting gesture is recognized by the same capture gate.
    #[test]
    fn dictation_agent_and_assistive_all_start_shared_capture() {
        for event in [
            HotkeyEvent::ToggleNormal,
            HotkeyEvent::ToggleAssistive,
            HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Raw,
            },
            HotkeyEvent::Hold {
                action: HoldAction::Down,
                mode: HoldMode::Chat,
            },
        ] {
            assert!(
                event_can_start_capture(&event),
                "missing shared start for {event:?}"
            );
        }
    }

    /// ShowAgent remains UI-only; recording gestures all reach the same callback.
    #[test]
    fn agent_notification_does_not_own_capture_or_transcript_payload() {
        let listener = Arc::new(CountingAppActionListener::new());
        let recording_calls = Arc::new(AtomicUsize::new(0));
        let recording_calls_for_route = Arc::clone(&recording_calls);

        route_hotkey_event(
            HotkeyEvent::ShowAgent,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("show agent must not dispatch deferred insert"),
        );

        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 0);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::ToggleNormal,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("recording command must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 1);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::ToggleAssistive,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("assistive command must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 2);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 2);

        let deferred_calls = Arc::new(AtomicUsize::new(0));
        let deferred_calls_for_route = Arc::clone(&deferred_calls);
        route_hotkey_event(
            HotkeyEvent::InsertHere,
            Some(listener.clone()),
            |_| panic!("deferred insert must not enter recording dispatch"),
            move || {
                deferred_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(deferred_calls.load(Ordering::SeqCst), 1);
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 2);

        route_hotkey_event(
            HotkeyEvent::ToggleWidget,
            Some(listener.clone()),
            |_| panic!("widget toggle must not enter recording dispatch"),
            || panic!("widget toggle must not dispatch deferred insert"),
        );
        assert_eq!(listener.toggle_widget_calls.load(Ordering::SeqCst), 1);
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 2);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn mid_hold_attach_does_not_target_agent_or_claim_capture() {
        assert!(!event_can_start_capture(&HotkeyEvent::AttachSelection));
        assert!(
            !event_can_start_capture(&HotkeyEvent::AgentChannel { digit: 0 }),
            "Fn+digit must not take the dictation capture gate"
        );
        assert!(!event_targets_agent_ui(&HotkeyEvent::AttachSelection));
        assert!(!event_targets_agent_ui(&HotkeyEvent::HoldUpdate {
            mode: HoldMode::Chat,
        }));
        assert!(!event_targets_agent_ui(&HotkeyEvent::Hold {
            action: HoldAction::Up,
            mode: HoldMode::Chat,
        }));

        let listener = Arc::new(CountingAppActionListener::new());
        let recording_calls = Arc::new(AtomicUsize::new(0));
        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::AttachSelection,
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("attach must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 1);

        let recording_calls_for_route = Arc::clone(&recording_calls);
        route_hotkey_event(
            HotkeyEvent::HoldUpdate {
                mode: HoldMode::Chat,
            },
            Some(listener.clone()),
            move |_| {
                recording_calls_for_route.fetch_add(1, Ordering::SeqCst);
            },
            || panic!("hold update must not dispatch deferred insert"),
        );
        assert_eq!(listener.show_agent_calls.load(Ordering::SeqCst), 0);
        assert_eq!(recording_calls.load(Ordering::SeqCst), 2);
    }
}

#[cfg(test)]
mod composer_stop_bridge_tests {
    use super::*;

    #[test]
    fn pending_and_unavailable_are_preserved_by_the_production_mapping() {
        for (source, target) in [
            (CaptureStopOutcome::Stopped, CsConditionalStop::Stopped),
            (
                CaptureStopOutcome::ForeignCapture,
                CsConditionalStop::ForeignCapture,
            ),
            (
                CaptureStopOutcome::NoLiveCapture,
                CsConditionalStop::NoLiveCapture,
            ),
            (
                CaptureStopOutcome::AlreadyStopping,
                CsConditionalStop::AlreadyStopping,
            ),
            (CaptureStopOutcome::Pending, CsConditionalStop::Pending),
            (
                CaptureStopOutcome::AdmissionUnavailable,
                CsConditionalStop::AdmissionUnavailable,
            ),
        ] {
            assert_eq!(CsConditionalStop::from(source), target);
        }
    }

    #[test]
    fn held_controller_store_refuses_without_waiting_or_creating_a_controller() {
        let store: SharedController = Arc::new(Mutex::new(None));
        let held = store.lock().unwrap();
        let mut call = Box::pin(stop_composer_capture(
            &store,
            CsCaptureHandle {
                capture_id: "mine".to_string(),
            },
        ));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let std::task::Poll::Ready(result) = call.as_mut().poll(&mut context) else {
            panic!("named bridge Stop must refuse before suspension on a held store");
        };
        assert_eq!(result.unwrap(), CsConditionalStop::AdmissionUnavailable);
        assert!(held.is_none());
    }

    #[tokio::test]
    async fn absent_controller_and_idle_real_controller_report_no_live_capture() {
        let store: SharedController = Arc::new(Mutex::new(None));
        assert_eq!(
            stop_composer_capture(
                &store,
                CsCaptureHandle {
                    capture_id: "mine".to_string()
                }
            )
            .await
            .unwrap(),
            CsConditionalStop::NoLiveCapture
        );
        let controller = fixture_controller();
        *store.lock().unwrap() = Some(Arc::clone(&controller));
        for _ in 0..2 {
            assert_eq!(
                stop_composer_capture(
                    &store,
                    CsCaptureHandle {
                        capture_id: "mine".to_string()
                    }
                )
                .await
                .unwrap(),
                CsConditionalStop::NoLiveCapture
            );
            assert_eq!(controller.current_state().await, State::Idle);
        }
    }
}

/// Product-owned seal-lane provenance as exposed by the admission bridge.
#[cfg(test)]
mod admission_readiness_source_tests {
    use super::*;
    use serial_test::serial;

    use codescribe_core::test_isolation::EnvGuard;

    /// Scratch product settings own the bridge source until process env
    /// explicitly overrides the same immutable snapshot input.
    #[test]
    #[serial]
    fn scratch_settings_and_env_override_project_honest_admission_source() {
        const SEAL_LANE_ENV: &str = codescribe_core::pipeline::streaming::SILERO_FUSION_ENV;

        let scratch = tempfile::tempdir().expect("create scratch settings root");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", scratch.path());
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _seal_lane_override = EnvGuard::remove(SEAL_LANE_ENV);

        UserSettings {
            seal_lane_armed: Some(true),
            ..Default::default()
        }
        .save()
        .expect("write scratch product settings");

        let settings_path = UserSettings::settings_path();
        let settings_root = settings_path
            .parent()
            .expect("settings path has a parent")
            .canonicalize()
            .expect("canonical persisted settings root");
        assert_eq!(
            settings_root,
            scratch
                .path()
                .canonicalize()
                .expect("canonical scratch settings root"),
            "settings write must remain inside the scratch data root"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&settings_path).expect("read scratch settings.json"),
        )
        .expect("parse scratch settings.json");
        assert_eq!(
            persisted.pointer("/audio/seal_lane_armed"),
            Some(&serde_json::Value::Bool(true))
        );

        let settings_snapshot = Config::load_runtime_snapshot_without_keychain()
            .expect("load settings-owned runtime snapshot");
        let settings_readiness = project_admission_readiness(
            &settings_snapshot,
            Err(admission::AdmissionBlocker::SealVadUnavailable),
        );
        assert!(settings_readiness.seal_lane_armed);
        assert!(settings_readiness.seal_lane_setting_armed);
        assert_eq!(settings_readiness.seal_lane_source, "settings");

        // SAFETY: the test remains under the same global serial_test lock and
        // `_seal_lane_override` restores the inherited value on every exit.
        unsafe { std::env::set_var(SEAL_LANE_ENV, "0") };
        let override_snapshot = Config::load_runtime_snapshot_without_keychain()
            .expect("load env-overridden runtime snapshot");
        let override_readiness = project_admission_readiness(
            &override_snapshot,
            Err(admission::AdmissionBlocker::SealVadUnavailable),
        );
        assert!(!override_readiness.seal_lane_armed);
        assert!(override_readiness.seal_lane_setting_armed);
        assert_eq!(override_readiness.seal_lane_source, "env_override");
    }
}

/// The Settings mode-binding surface: FFI enum roundtrips, the re-arm gate,
/// conflict validation, and a persist/read-back cycle against an isolated
/// `CODESCRIBE_DATA_DIR`.
#[cfg(test)]
mod mode_binding_tests {
    use super::*;
    use serial_test::serial;
    use std::process::Command;
    use std::sync::Mutex;

    /// Serializes `CODESCRIBE_DATA_DIR` mutation for the persist/read-back test.
    // Serializes the CODESCRIBE_DATA_DIR-mutating test below within this module.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RED contract for the production-log pollution observed from this test
    /// module. A child test process gives `init_logging` a fresh `Once`, a fake
    /// HOME, and a distinct test data root; initialization must not create the
    /// production-shaped `~/.codescribe/data.noindex/logs/codescribe.log` sink.
    #[test]
    #[serial]
    fn fleet_red_test_logging_isolated() {
        const CHILD_ENV: &str = "CODESCRIBE_FLEET_RED_LOG_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let production_path = std::path::PathBuf::from(
                std::env::var_os("HOME").expect("isolated child HOME must be set"),
            )
            .join(".codescribe")
            .join("data.noindex")
            .join("logs")
            .join("codescribe.log");

            codescribe::logging::init_logging();
            assert!(
                !production_path.exists(),
                "test logger initialization resolved to production path: {}",
                production_path.display()
            );
            return;
        }

        let fake_home = tempfile::tempdir().expect("create isolated HOME");
        let test_data = tempfile::tempdir().expect("create isolated test data root");
        let status = Command::new(std::env::current_exe().expect("resolve test binary"))
            .args([
                "--exact",
                "hotkeys::mode_binding_tests::fleet_red_test_logging_isolated",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("HOME", fake_home.path())
            .env("CODESCRIBE_DATA_DIR", test_data.path())
            .status()
            .expect("launch isolated logging child");

        assert!(
            status.success(),
            "isolated logger child must avoid the production log path"
        );
    }

    /// Every core work mode survives a UniFFI round-trip without loss.
    #[test]
    fn work_mode_ffi_round_trips() {
        for mode in ALL_WORK_MODES {
            let cs: CsWorkMode = mode.into();
            assert_eq!(WorkMode::from(cs), mode);
        }
    }

    /// Re-arm rebuilds only when the tap is down and both TCC gates are granted.
    #[test]
    fn rearm_gate_rebuilds_only_when_inactive_and_fully_granted() {
        use PermissionStatus::{Denied, Granted, NotDetermined};

        // The one case that arms: tap not yet live, both gating perms granted.
        assert!(should_rearm_hotkey_tap(false, Granted, Granted));

        // Dedup: an already-live tap is never torn down, even fully granted.
        assert!(!should_rearm_hotkey_tap(true, Granted, Granted));

        // Missing either gating permission must not trigger a doomed rebuild.
        assert!(!should_rearm_hotkey_tap(false, Denied, Granted));
        assert!(!should_rearm_hotkey_tap(false, Granted, Denied));
        assert!(!should_rearm_hotkey_tap(
            false,
            NotDetermined,
            NotDetermined
        ));

        // Already active + missing perms is still a no-op (both guards agree).
        assert!(!should_rearm_hotkey_tap(true, Denied, Denied));
    }

    /// Every closed-set gesture survives a UniFFI round-trip without loss.
    #[test]
    fn shortcut_binding_ffi_round_trips() {
        for binding in ALL_SHORTCUT_BINDINGS {
            let cs: CsShortcutBinding = binding.into();
            assert_eq!(ShortcutBinding::from(cs), binding);
        }
    }

    /// Settings picker options match `ALL_SHORTCUT_BINDINGS` length and labels.
    #[test]
    fn available_bindings_cover_the_closed_set() {
        let options = CodescribeHotkeys::new().available_bindings();
        assert_eq!(options.len(), ALL_SHORTCUT_BINDINGS.len());
        for (option, expected) in options.iter().zip(ALL_SHORTCUT_BINDINGS.iter()) {
            assert_eq!(ShortcutBinding::from(option.binding), *expected);
            assert!(!option.label.is_empty());
        }
    }

    /// Build a three-mode candidate binding set for `validate_bindings` tests.
    fn candidate(
        dictation: CsShortcutBinding,
        formatting: CsShortcutBinding,
        assistive: CsShortcutBinding,
    ) -> Vec<CsModeBinding> {
        vec![
            build_mode_binding(WorkMode::Dictation, dictation.into()),
            build_mode_binding(WorkMode::Formatting, formatting.into()),
            build_mode_binding(WorkMode::Assistive, assistive.into()),
        ]
    }

    /// Reachability refusals are blocking: a hold on Formatting never routes.
    #[test]
    fn validate_flags_internal_reachability_conflict_as_blocking() {
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::HoldFn,
            CsShortcutBinding::HoldCtrl,
            CsShortcutBinding::Disabled,
        ));
        assert!(
            conflicts.iter().any(|c| c.blocking
                && c.message
                    == "This gesture only starts Dictation, so Formatting would never start from it."),
            "a known reachability collision must surface a blocking conflict, got {conflicts:?}"
        );
    }

    /// Double Ctrl dictation next to opt-in Option gestures is a legal profile:
    /// none of Codescribe's own rules flags it.
    #[test]
    fn validate_allows_double_ctrl_dictation_with_option_gestures() {
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::DoubleCtrl,
            CsShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::DoubleRightOption,
        ));
        assert!(
            !conflicts.iter().any(|c| c.blocking
                && (c.message.starts_with("This gesture only starts")
                    || c.message.starts_with("This gesture already starts"))),
            "opt-in Option gestures are routed cells, got {conflicts:?}"
        );
    }

    /// Formatting and Assistive sharing one reachable gesture is a blocking
    /// duplicate that names Formatting as the owner.
    #[test]
    fn validate_blocks_duplicate_double_ctrl_naming_formatting() {
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::HoldFn,
            CsShortcutBinding::DoubleCtrl,
            CsShortcutBinding::DoubleCtrl,
        ));
        assert!(
            conflicts.iter().any(|c| c.blocking
                && c.message
                    == "This gesture already starts Formatting; one gesture starts one mode."),
            "duplicate gesture must block naming Formatting, got {conflicts:?}"
        );
    }

    /// A hold-only profile with disabled toggles validates with zero conflicts.
    #[test]
    fn validate_is_clean_for_a_safe_hold_only_profile() {
        // HoldCtrl never collides with macOS symbolic hotkeys and Disabled toggles
        // add nothing — a deterministic zero-conflict candidate on any machine.
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::HoldCtrl,
            CsShortcutBinding::Disabled,
            CsShortcutBinding::Disabled,
        ));
        assert!(
            conflicts.is_empty(),
            "safe hold-only profile must validate clean, got {conflicts:?}"
        );
    }

    /// Dictation on an Option double-tap is a blocking conflict with the
    /// Dictation sentence, in the same validation the Settings Save button is
    /// gated on.
    #[test]
    fn validate_blocks_dictation_on_option_gesture() {
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::DoubleRightOption,
        ));
        assert!(
            conflicts.iter().any(|c| c.blocking
                && c.gesture_label == "Double-tap Left Option"
                && c.message
                    == "This gesture only starts Formatting or Assistive, so Dictation would never start from it."),
            "Dictation on left Option must block, got {conflicts:?}"
        );
    }

    /// The Fn-tap note is informational on every machine: whatever the macOS
    /// registry says, the default profile carries no blocking entry from
    /// Codescribe's own rules, and a note never blocks.
    #[test]
    fn validate_defaults_never_block_on_the_fn_tap_note() {
        let conflicts = CodescribeHotkeys::new().validate_bindings(candidate(
            CsShortcutBinding::HoldFn,
            CsShortcutBinding::DoubleLeftOption,
            CsShortcutBinding::DoubleRightOption,
        ));
        for conflict in conflicts
            .iter()
            .filter(|c| c.gesture_label == "Hold Fn/Globe")
        {
            if conflict
                .message
                .starts_with("Fn/Globe tap is configured by macOS")
            {
                assert!(!conflict.blocking, "the Fn-tap note must not block Save");
            }
        }
        assert!(
            !conflicts
                .iter()
                .any(|c| c.blocking && c.message.starts_with("This gesture only starts")),
            "defaults are routed cells, got {conflicts:?}"
        );
    }

    /// Persist/read-back cycle against an isolated `CODESCRIBE_DATA_DIR`.
    #[test]
    #[serial]
    fn set_mode_binding_persists_and_reads_back_through_the_bridge() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!("cs_bridge_hotkeys_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create isolated data dir");
        let previous = std::env::var("CODESCRIBE_DATA_DIR").ok();
        // SAFETY: serialized by ENV_LOCK; env is restored before the lock drops.
        unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", &dir) };

        let hotkeys = CodescribeHotkeys::new();
        hotkeys
            .set_mode_binding(CsWorkMode::Dictation, CsShortcutBinding::HoldCtrlAlt)
            .expect("set_mode_binding");

        let bindings = hotkeys.get_mode_bindings();
        let dictation = bindings
            .iter()
            .find(|b| b.mode == CsWorkMode::Dictation)
            .expect("dictation binding present");
        assert_eq!(dictation.binding, CsShortcutBinding::HoldCtrlAlt);

        // Unrouted cells are refused by the setter and never reach disk.
        // Formatting and Assistive default to Disabled; Option gestures are
        // opt-in and only Formatting/Assistive accept them.
        let binding_of = |mode: CsWorkMode| {
            hotkeys
                .get_mode_bindings()
                .into_iter()
                .find(|b| b.mode == mode)
                .map(|b| b.binding)
        };
        assert_eq!(
            binding_of(CsWorkMode::Formatting),
            Some(CsShortcutBinding::Disabled)
        );
        assert_eq!(
            binding_of(CsWorkMode::Assistive),
            Some(CsShortcutBinding::Disabled)
        );
        let refused = hotkeys
            .set_mode_binding(CsWorkMode::Dictation, CsShortcutBinding::DoubleLeftOption)
            .expect_err("dictation never starts from double-tap left Option");
        assert!(
            format!("{refused:?}").contains("only starts Formatting or Assistive"),
            "{refused:?}"
        );
        assert_eq!(
            binding_of(CsWorkMode::Dictation),
            Some(CsShortcutBinding::HoldCtrlAlt)
        );
        hotkeys
            .set_mode_binding(CsWorkMode::Formatting, CsShortcutBinding::DoubleLeftOption)
            .expect("Formatting opts in to double-tap left Option");
        hotkeys
            .set_mode_binding(CsWorkMode::Assistive, CsShortcutBinding::DoubleRightOption)
            .expect("Assistive opts in to double-tap right Option");
        for (mode, binding) in [
            (CsWorkMode::Formatting, CsShortcutBinding::HoldCtrl),
            (CsWorkMode::Assistive, CsShortcutBinding::HoldCtrlCmd),
        ] {
            assert!(
                hotkeys.set_mode_binding(mode, binding).is_err(),
                "{mode:?} x {binding:?} must be refused"
            );
        }
        assert_eq!(
            binding_of(CsWorkMode::Formatting),
            Some(CsShortcutBinding::DoubleLeftOption)
        );
        assert_eq!(
            binding_of(CsWorkMode::Assistive),
            Some(CsShortcutBinding::DoubleRightOption)
        );
        hotkeys
            .set_mode_binding(CsWorkMode::Assistive, CsShortcutBinding::DoubleCtrl)
            .expect("double-tap Ctrl is routed for any mode");
        assert_eq!(
            binding_of(CsWorkMode::Assistive),
            Some(CsShortcutBinding::DoubleCtrl)
        );
        hotkeys
            .set_mode_binding(CsWorkMode::Dictation, CsShortcutBinding::DoubleCtrl)
            .expect("dictation double-tap Ctrl is routed");
        assert_eq!(
            binding_of(CsWorkMode::Dictation),
            Some(CsShortcutBinding::DoubleCtrl)
        );

        // Reset restores defaults through the same path.
        hotkeys
            .reset_bindings_to_defaults()
            .expect("reset_bindings_to_defaults");
        let after_reset = hotkeys.get_mode_bindings();
        let dictation_reset = after_reset
            .iter()
            .find(|b| b.mode == CsWorkMode::Dictation)
            .expect("dictation binding present after reset");
        assert_eq!(dictation_reset.binding, CsShortcutBinding::HoldFn);
        for mode in [CsWorkMode::Formatting, CsWorkMode::Assistive] {
            let reset = after_reset
                .iter()
                .find(|b| b.mode == mode)
                .expect("binding present after reset");
            assert_eq!(reset.binding, CsShortcutBinding::Disabled, "{mode:?}");
        }

        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Paste-result mapping across the FFI boundary: app names normalize to `None`
/// when blank, and a permission denial keeps both observed app names intact.
#[cfg(test)]
mod paste_target_mapping_tests {
    use super::{CsPasteOutcome, CsPasteResult, normalize_paste_target_app_name};
    use codescribe::controller::{OverlayPasteDelivery, OverlayPasteResult};

    /// Non-empty app names trim and stay present across the bridge mapping.
    #[test]
    fn bridge_mapping_keeps_present_app_name() {
        assert_eq!(
            normalize_paste_target_app_name(Some("  Ghostty  ".to_string())).as_deref(),
            Some("Ghostty")
        );
    }

    /// None / whitespace-only names collapse to absent for Swift labels.
    #[test]
    fn bridge_mapping_returns_absent_for_unknown_or_empty_name() {
        assert_eq!(normalize_paste_target_app_name(None), None);
        assert_eq!(
            normalize_paste_target_app_name(Some("   ".to_string())),
            None
        );
    }

    /// Accessibility denial keeps observed app names intact on the FFI result.
    #[test]
    fn accessibility_denial_maps_with_delivery_app_truth() {
        let result = CsPasteResult::from(OverlayPasteResult {
            delivery: OverlayPasteDelivery::AccessibilityPermissionNeeded,
            target_app_name: Some("Pensieve".to_string()),
            frontmost_app_name: Some("Pensieve".to_string()),
            deferred_insert_shortcut: None,
            deferred_insert_failure: None,
        });

        assert_eq!(
            result.outcome,
            CsPasteOutcome::AccessibilityPermissionNeeded
        );
        assert_eq!(result.target_app_name.as_deref(), Some("Pensieve"));
        assert_eq!(result.frontmost_app_name.as_deref(), Some("Pensieve"));
    }
}

// ===========================================================================
// Orphaned optimistic-overlay compensation (CUT P0a)
//
// Contract under test: any dispatch that shows the optimistic "preparing"
// overlay is guaranteed a terminal listener event. When the controller ends the
// dispatch back at Idle WITHOUT a StateChange broadcast — the shape produced by
// the quick hold-release cancel (`cancel_pending_hold_start`), the start-failure
// reset (`reset_session_after_start_failure` → `set_state(Idle)` at old==Idle),
// and the no-op re-check dispatch — `compensate_orphaned_preparing` emits exactly
// one compensating `on_recording_stopped`. When a real transition occurred the
// broadcast forwarder owns the terminal event and the compensator must NOT
// double-fire.
// ===========================================================================
/// The terminal-event guarantee for the optimistic overlay described in the
/// banner above, plus the `busy` → finalising → `idle` → stopped forwarding
/// sequence a real hold-release produces.
#[cfg(test)]
mod preparing_compensation_tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Mutex as AsyncMutex;

    /// Async mutex serializing process-global PREPARING/listener/controller state.
    // Serializes the process-global PREPARING_PENDING / shared_listener /
    // shared_controller these tests mutate, so parallel runs don't interleave.
    // Async-aware so the guard can be held across the `.await` points below.
    static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    /// Capturing transcription listener for optimistic-overlay compensation tests.
    #[derive(Default)]
    struct RecordingLifecycleListener {
        preparing: AtomicUsize,
        started: AtomicUsize,
        stopped: AtomicUsize,
        finalising: AtomicUsize,
        audio_levels: StdMutex<Vec<f32>>,
        compact_paints: StdMutex<Vec<crate::recording::CsCompactProjection>>,
    }

    impl RecordingLifecycleListener {
        /// Count of `on_recording_preparing` callbacks observed.
        fn preparing(&self) -> usize {
            self.preparing.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_started` callbacks observed.
        fn started(&self) -> usize {
            self.started.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_stopped` callbacks observed.
        fn stopped(&self) -> usize {
            self.stopped.load(Ordering::SeqCst)
        }
        /// Count of `on_recording_finalising` callbacks observed.
        fn finalising(&self) -> usize {
            self.finalising.load(Ordering::SeqCst)
        }
        /// Snapshot of RMS audio-level samples the listener captured.
        fn audio_levels(&self) -> Vec<f32> {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl CsTranscriptionListener for RecordingLifecycleListener {
        /// No-op: transcript projections are not under test in this suite.
        fn on_transcript_projection(&self, _event: CsTranscriptProjectionEvent) {}
        /// No-op: passive presentation statuses are not under test in this suite.
        fn on_presentation_status(&self, _event: CsPresentationStatusEvent) {}
        fn on_compact_projection(&self, event: crate::recording::CsCompactProjection) {
            self.compact_paints.lock().unwrap().push(event);
        }
        /// Count preparing overlay shows for compensation assertions.
        fn on_recording_preparing(&self) {
            self.preparing.fetch_add(1, Ordering::SeqCst);
        }
        /// Count real start transitions from the broadcast forwarder.
        fn on_recording_started(&self) {
            self.started.fetch_add(1, Ordering::SeqCst);
        }
        /// Count terminal stop events (broadcast or compensating).
        fn on_recording_stopped(&self) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
        /// Count busy→finalising transitions for the hold-release sequence.
        fn on_recording_finalising(&self) {
            self.finalising.fetch_add(1, Ordering::SeqCst);
        }
        /// No-op: session finalisation summary is not under test here.
        fn on_session_finalised(&self, _session_id: String, _layer_summary: CsLayerSummary) {}
        /// No-op: VAD active toggles are not under test in this suite.
        fn on_vad_active(&self, _active: bool) {}
        /// Capture RMS samples so audio-level forwarding can be asserted.
        fn on_audio_level(&self, rms: f32) {
            self.audio_levels
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(rms);
        }
        /// No-op: no-speech reasons are not under test in this suite.
        fn on_no_speech(&self, _reason: String) {}
        /// No-op: error messages are not under test in this suite.
        fn on_error(&self, _message: String) {}
    }

    /// Install a fresh capturing listener + an Idle controller into the shared
    /// process stores and clear the pending flag. Returns both so the test can
    /// assert on the listener and pass the controller to the compensator.
    fn install() -> (Arc<RecordingLifecycleListener>, Arc<RecordingController>) {
        PREPARING_PENDING.store(false, Ordering::SeqCst);
        let listener = Arc::new(RecordingLifecycleListener::default());
        *shared_listener().write().unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>);
        let controller = fixture_controller();
        *shared_controller()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&controller));
        (listener, controller)
    }

    /// Clear shared listener/controller stores and the preparing flag after a test.
    fn teardown() {
        *shared_listener().write().unwrap_or_else(|e| e.into_inner()) = None;
        *shared_controller()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        PREPARING_PENDING.store(false, Ordering::SeqCst);
    }

    /// Calling synchronously while this very thread owns the slot would hang
    /// with a blocking acquisition. It must refuse without constructing a
    /// controller, and a refused attempt must leave later idle admission open.
    #[tokio::test]
    #[serial]
    async fn idle_restart_bridge_refuses_absence_and_slot_contention_then_reserves_idle() {
        let _guard = TEST_LOCK.lock().await;
        teardown();
        let hotkeys = CodescribeHotkeys::default();
        assert!(!hotkeys.reserve_idle_restart().unwrap());
        assert!(shared_controller().lock().unwrap().is_none());
        let (_listener, _controller) = install();
        let store = shared_controller();
        let held_slot = store.lock().unwrap();
        assert!(!hotkeys.reserve_idle_restart().unwrap());
        drop(held_slot);
        assert!(hotkeys.reserve_idle_restart().unwrap());
        assert!(!hotkeys.reserve_idle_restart().unwrap());
        teardown();
    }

    /// A host lifecycle notification while idle must not construct the shared
    /// controller (and therefore cannot prewarm or load an engine).
    #[tokio::test]
    #[serial]
    async fn sleep_wake_without_active_controller_is_a_noop() {
        let _guard = TEST_LOCK.lock().await;
        teardown();

        let hotkeys = CodescribeHotkeys::new();
        assert!(!hotkeys.note_sleep_wake().await);
        assert!(current_controller(&shared_controller()).is_none());
    }

    /// The compact paint crosses IPC with its capture identity and its
    /// read-only evidence intact; an invalid payload is rejected.
    #[test]
    fn compact_projection_transport_preserves_identity_and_rejects_invalid_json() {
        let listener = Arc::new(RecordingLifecycleListener::default());
        let paint = codescribe::presentation::emitter::CompactProjection {
            session_id: "take".into(),
            capture_epoch: 7,
            sequence: 2,
            text: "…".into(),
            degraded: true,
            pending_ranges: vec![codescribe::presentation::emitter::PendingRange {
                utf16_start: 0,
                utf16_end: 1,
            }],
            voice: codescribe::presentation::emitter::CursorVoice::Lagging,
            evidence: vec![codescribe::presentation::emitter::UnanchoredEvidence {
                sample_start: 16_000,
                sample_end: 32_000,
                text: "inna wersja".into(),
                reason: "exclusive_tail_awaiting_whole_span".into(),
            }],
        };
        forward_event_to_listener(
            IpcEventPayload::CompactProjection {
                json: serde_json::to_string(&paint).unwrap(),
            },
            listener.clone(),
        );
        forward_event_to_listener(
            IpcEventPayload::CompactProjection { json: "{}".into() },
            listener.clone(),
        );
        let received = listener.compact_paints.lock().unwrap().clone();
        assert_eq!(received, vec![paint.into()]);
        assert_eq!(received[0].evidence[0].text, "inna wersja");
        assert_eq!(received[0].evidence[0].sample_end, 32_000);
        assert_eq!(listener.started(), 0);
        assert_eq!(listener.stopped(), 0);
    }

    #[test]
    fn recording_audio_level_payload_forwards_rms() {
        let listener = Arc::new(RecordingLifecycleListener::default());
        forward_event_to_listener(
            IpcEventPayload::AudioLevel { rms: 0.125 },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(listener.audio_levels(), vec![0.125]);
    }

    /// Paths 1 & 2 (quick hold-release cancel, start-failure reset): preparing was
    /// shown, the controller ended the dispatch at Idle with no broadcast → the
    /// compensator must emit exactly one terminal stop.
    #[tokio::test]
    #[serial]
    async fn orphaned_preparing_at_idle_gets_a_compensating_stop() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        // The optimistic overlay is shown for a start gesture at Idle.
        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        assert_eq!(listener.preparing(), 1, "preparing overlay must be shown");
        assert!(PREPARING_PENDING.load(Ordering::SeqCst), "flag armed");

        // The dispatch left the controller at Idle without any StateChange
        // (the shape of cancel_pending_hold_start / start-failure reset).
        compensate_orphaned_preparing(&controller).await;

        assert_eq!(
            listener.stopped(),
            1,
            "orphaned preparing must receive one terminal stop"
        );
        assert!(!PREPARING_PENDING.load(Ordering::SeqCst), "flag cleared");
        teardown();
    }

    /// The compensator is inert when no optimistic overlay was shown: an ordinary
    /// stop dispatch (controller back at Idle, but flag never armed) must not have a
    /// spurious extra stop synthesized on top of the broadcast one.
    #[tokio::test]
    #[serial]
    async fn no_preparing_shown_means_no_compensating_stop() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        compensate_orphaned_preparing(&controller).await;

        assert_eq!(listener.preparing(), 0);
        assert_eq!(
            listener.stopped(),
            0,
            "no preparing was pending, so nothing to compensate"
        );
        teardown();
    }

    /// Idempotency: a second compensator pass (e.g. the FFI `start_recording` path
    /// racing the hotkey spawn) must not emit a second stop for the same overlay.
    #[tokio::test]
    #[serial]
    async fn compensation_is_idempotent_across_repeated_passes() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        compensate_orphaned_preparing(&controller).await;
        compensate_orphaned_preparing(&controller).await;

        assert_eq!(
            listener.stopped(),
            1,
            "the compensating stop must fire at most once per preparing"
        );
        teardown();
    }

    /// Path 3 (no-op dispatch) / genuine start: when a real transition's broadcast
    /// already resolved the preparing (forwarder cleared the flag and emitted
    /// started), the compensator must not double-fire a stop on top of it.
    #[tokio::test]
    #[serial]
    async fn forwarder_resolved_preparing_is_not_double_stopped() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, controller) = install();

        optimistically_show_overlay(&HotkeyEvent::ToggleNormal).await;
        assert!(PREPARING_PENDING.load(Ordering::SeqCst));

        // Simulate the broadcast forwarder observing a real Idle→rec_toggle
        // transition: it emits started and clears the pending flag.
        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "idle".to_string(),
                to: "rec_toggle".to_string(),
            },
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        assert_eq!(listener.started(), 1, "forwarder emitted started");
        assert!(
            !PREPARING_PENDING.load(Ordering::SeqCst),
            "forwarder cleared flag"
        );

        // A late compensator pass (controller now back at Idle) must stay silent —
        // the started already resolved the overlay.
        compensate_orphaned_preparing(&controller).await;
        assert_eq!(
            listener.stopped(),
            0,
            "a forwarder-resolved preparing must not be double-stopped"
        );
        teardown();
    }

    /// The `Busy` StateChange (final transcription pass, after capture ends) routes
    /// to `on_recording_finalising` — the native-path signal that lets the overlay
    /// enter its "transcribing" phase — and NOT to started/stopped. The terminal
    /// `idle` still maps to `on_recording_stopped`, so the sequence a real
    /// hold-release / toggle stop produces (rec_hold → busy → idle) yields exactly
    /// one finalising then one stopped.
    #[tokio::test]
    #[serial]
    async fn busy_state_routes_to_finalising_then_idle_to_stopped() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();
        let dyn_listener = || Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>;

        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "rec_hold".to_string(),
                to: "busy".to_string(),
            },
            dyn_listener(),
        );
        assert_eq!(listener.finalising(), 1, "busy → finalising");
        assert_eq!(listener.stopped(), 0, "busy must not fire stopped");
        assert_eq!(listener.started(), 0, "busy must not fire started");

        forward_event_to_listener(
            IpcEventPayload::StateChange {
                from: "busy".to_string(),
                to: "idle".to_string(),
            },
            dyn_listener(),
        );
        assert_eq!(listener.stopped(), 1, "idle → stopped");
        assert_eq!(listener.finalising(), 1, "idle must not re-fire finalising");
        teardown();
    }

    /// A lag recovery does not replay dropped payloads. It snapshots controller
    /// truth instead; when that truth is Idle, native capture ownership and the
    /// overlay lifecycle must both end even if the original idle broadcast was
    /// among the dropped messages.
    #[tokio::test]
    #[serial]
    async fn authoritative_idle_reconciliation_releases_native_capture() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();
        CAPTURE_OWNER.store(CAPTURE_OWNER_CONTROLLER, Ordering::SeqCst);

        forward_controller_state_to_listener(
            State::Idle,
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );
        release_controller_capture_owner_if_idle(State::Idle);

        assert_eq!(listener.stopped(), 1, "authoritative Idle must stop UI");
        assert_eq!(
            CAPTURE_OWNER.load(Ordering::SeqCst),
            CAPTURE_OWNER_NONE,
            "authoritative Idle must release the microphone gate"
        );
        teardown();
    }

    /// Reconciliation also preserves non-terminal controller truth instead of
    /// flattening every loss event into a stop.
    #[tokio::test]
    #[serial]
    async fn authoritative_busy_reconciliation_keeps_finalising_phase() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();

        forward_controller_state_to_listener(
            State::Busy,
            Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>,
        );

        assert_eq!(listener.finalising(), 1);
        assert_eq!(listener.stopped(), 0);
        assert_eq!(listener.started(), 0);
        teardown();
    }

    /// A repeated `Busy` broadcast forwards a second `on_recording_finalising`; the
    /// idempotency that matters (a no-op re-entry) lives in the Swift handler, so
    /// the forwarder stays a thin, stateless router here.
    #[tokio::test]
    #[serial]
    async fn repeated_busy_forwards_each_finalising() {
        let _guard = TEST_LOCK.lock().await;
        let (listener, _controller) = install();
        let dyn_listener = || Arc::clone(&listener) as Arc<dyn CsTranscriptionListener>;

        for _ in 0..2 {
            forward_event_to_listener(
                IpcEventPayload::StateChange {
                    from: "rec_hold".to_string(),
                    to: "busy".to_string(),
                },
                dyn_listener(),
            );
        }
        assert_eq!(listener.finalising(), 2, "each busy forwards a finalising");
        assert!(
            !PREPARING_PENDING.load(Ordering::SeqCst),
            "busy must not arm the preparing flag"
        );
        teardown();
    }
}
