use super::super::speech_clock::VOICELESS_CLOSE_SECS;
use super::channel_transcription_controls_tests::measured_speech_end;
use super::*;
use serial_test::serial;
use std::time::{Duration, SystemTime};

fn write_binding(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join(BINDING_FILENAME);
    std::fs::write(&path, body).expect("write binding");
    path
}

fn chord_error(refusal: ChannelOpenRefusal) -> anyhow::Error {
    super::refusal(refusal)
}

#[test]
fn a_chord_on_a_vacant_digit_earns_the_refusal_sound_and_a_pointer_flash() {
    for vacant in [
        ChannelOpenRefusal::DigitUnbound { digit: 7 },
        ChannelOpenRefusal::BindingMissing {
            path: "fixture.json".into(),
        },
    ] {
        let feedback =
            channel_refusal_feedback(7, &chord_error(vacant.clone()), ChannelOpenMode::Live, true);
        assert_eq!(feedback.cue, Some("Basso"), "{vacant:?}");
        assert_eq!(
            feedback.cursor_label.as_deref(),
            Some("channel 7 empty"),
            "{vacant:?}"
        );
    }
    // The pointer preview keeps only the last three words of a label.
    assert_eq!(channel_refusal_label(7).split_whitespace().count(), 3);
}

#[test]
fn pointer_indicator_off_keeps_the_sound_and_drops_the_flash() {
    let feedback = channel_refusal_feedback(
        4,
        &chord_error(ChannelOpenRefusal::DigitUnbound { digit: 4 }),
        ChannelOpenMode::Live,
        false,
    );
    assert_eq!(feedback.cue, Some("Basso"));
    assert_eq!(feedback.cursor_label, None);
}

#[test]
fn broken_bindings_other_failures_and_attached_opens_get_no_feedback() {
    let silent = ChannelRefusalFeedback {
        cue: None,
        cursor_label: None,
    };
    for error in [
        chord_error(ChannelOpenRefusal::BindingMalformed {
            path: "fixture.json".into(),
            detail: "invalid JSON".into(),
        }),
        chord_error(ChannelOpenRefusal::InvalidDigit),
        anyhow::anyhow!("capture admission closed for shutdown"),
        // The refusal's text without its type is not the refusal.
        anyhow::anyhow!(ChannelOpenRefusal::DigitUnbound { digit: 2 }.to_string()),
    ] {
        assert_eq!(
            channel_refusal_feedback(2, &error, ChannelOpenMode::Live, true),
            silent,
            "{error}"
        );
    }
    assert_eq!(
        channel_refusal_feedback(
            2,
            &chord_error(ChannelOpenRefusal::DigitUnbound { digit: 2 }),
            ChannelOpenMode::AttachedOnly,
            true,
        ),
        silent,
        "an attached open is programmatic, not a user chord"
    );
}

fn bus_rows(bus: &Path) -> Vec<serde_json::Value> {
    crate::durable_bus_oracle::logical_text(bus)
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect()
}

/// Digit 3 bound to Leon on a dedicated `buses/channel-3.jsonl`.
fn write_dedicated_binding(dir: &Path) -> (PathBuf, PathBuf) {
    let channel_bus = dir.join("buses/channel-3.jsonl");
    let binding = format!(
        r#"{{"schema":"vc.agent-audience-binding.v1","bindings":{{"3":{{"audience":"Leon","provider":"codex","provider_session_id":"leon-session","bus":"{}"}}}}}}"#,
        channel_bus.display()
    );
    (write_binding(dir, &binding), channel_bus)
}

/// Opens digit 3 without a device and stamps the session label a Live
/// open mints, so receipts carry the identity followers key on. Returns
/// the open instant.
async fn open_stamped(
    controller: &RecordingController,
    binding: &Path,
    shared_bus: &Path,
    session: &str,
) -> SystemTime {
    controller
        .dispatch_agent_channel(3, binding, shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    stamp_session(controller, session).await
}

async fn stamp_session(controller: &RecordingController, session: &str) -> SystemTime {
    let mut channels = controller.agent_channels.lock().await;
    let open = channels.get_mut(&3).expect("channel 3 is open");
    // A one-message session: its document is the session.
    open.session_id = Some(session.to_string());
    open.channel_session_id = Some(session.to_string());
    open.opened_at
}

async fn hear_voice_at(controller: &RecordingController, at: SystemTime) {
    let open = controller.agent_channel_snapshot(3).await.expect("open");
    measured_speech_end(&open, at);
}

fn sealed_rows(rows: &[serde_json::Value]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|row| row["schema"] == "codescribe.channel-session.v1")
        .filter(|row| row["state"] == "sealed")
        .map(|row| {
            (
                row["session_id"].as_str().unwrap_or_default().to_string(),
                row["reason"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn cancelled_hangup_caller_retains_owned_capture_close_and_one_seal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    let controller = Arc::new(RecordingController::new_without_keychain());
    open_stamped(&controller, &binding, &shared_bus, "cancelled-stop").await;
    let mut open = controller
        .agent_channels
        .lock()
        .await
        .remove(&3)
        .expect("open");
    // Exercise the owned Live close transport without opening a device or ASR.
    open.mode = ChannelOpenMode::Live;
    let recorder_guard = controller.recorder.lock().await;
    let closing_controller = Arc::clone(&controller);
    let closing_bus = shared_bus.clone();
    let caller = tokio::spawn(async move {
        closing_controller
            .close_open_channel(3, open, ChannelSealReason::Hangup, &closing_bus)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !controller
                .closed_capture_tails
                .lock()
                .expect("tails")
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close owner retained before first wait");
    caller.abort();
    assert!(caller.await.expect_err("cancelled caller").is_cancelled());
    drop(recorder_guard);
    let tail = controller
        .closed_capture_tails
        .lock()
        .expect("tails")
        .pop()
        .expect("retained close");
    tokio::time::timeout(Duration::from_secs(2), tail)
        .await
        .expect("tail closes")
        .expect("tail joined");
    assert_eq!(controller.capture_subscriber_view().await.0, 0);
    assert_eq!(
        sealed_rows(&bus_rows(&channel_bus)),
        vec![("cancelled-stop".to_string(), "hangup".to_string())]
    );
    assert_eq!(ChannelSealReason::DictationHandoff.as_str(), "hangup");
    assert_eq!(ChannelSealReason::Silence.as_str(), "silence");
}

#[test]
fn digit_zero_is_broadcast_without_a_binding_file() {
    let missing = Path::new("/tmp/codescribe-fn1-missing-binding.json");
    let bound = resolve_digit(0, missing).expect("broadcast");
    assert_eq!(bound.audience, "*");
    assert!(bound.provider.is_none());
    const {
        assert!(!CHANNEL_PREVIEW.autopaste);
        assert!(!CHANNEL_PREVIEW.overlay_document);
        assert!(CHANNEL_PREVIEW.hold_badge);
    }
}

#[test]
fn missing_malformed_and_unbound_digits_refuse_loudly() {
    let dir = tempfile::tempdir().expect("temp");
    let missing = dir.path().join(BINDING_FILENAME);
    match resolve_digit(3, &missing) {
        Err(ChannelOpenRefusal::BindingMissing { path }) => {
            assert!(path.contains(BINDING_FILENAME), "{path}");
        }
        other => panic!("expected missing file, got {other:?}"),
    }

    let broken = write_binding(dir.path(), "{not json");
    match resolve_digit(3, &broken) {
        Err(ChannelOpenRefusal::BindingMalformed { detail, .. }) => {
            assert!(!detail.is_empty(), "{detail}");
        }
        other => panic!("expected malformed file, got {other:?}"),
    }

    let wrong_schema = write_binding(
        dir.path(),
        r#"{"schema":"other","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"s"}}}"#,
    );
    assert!(matches!(
        resolve_digit(3, &wrong_schema),
        Err(ChannelOpenRefusal::BindingMalformed { .. })
    ));

    let unbound = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude","provider_session_id":"lease-ada"}}}"#,
    );
    assert_eq!(
        resolve_digit(3, &unbound),
        Err(ChannelOpenRefusal::DigitUnbound { digit: 3 })
    );
    let leon = resolve_digit(1, &unbound).expect("bound session");
    assert_eq!(leon.audience, "Ada");
    assert_eq!(leon.provider.as_deref(), Some("claude"));
    assert_eq!(leon.provider_session_id.as_deref(), Some("lease-ada"));
    assert!(matches!(
        resolve_digit(10, &unbound),
        Err(ChannelOpenRefusal::InvalidDigit)
    ));
}

#[tokio::test]
async fn unbound_channel_refusal_retains_its_type_without_opening_capture() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{}}"#,
    );
    let bus = dir.path().join("shared-bus.jsonl");
    let error = controller
        .dispatch_agent_channel(2, &path, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect_err("unbound channel must refuse")
        .context("dispatching agent shortcut");
    assert_eq!(
        error.downcast_ref::<ChannelOpenRefusal>(),
        Some(&ChannelOpenRefusal::DigitUnbound { digit: 2 })
    );
    assert!(controller.agent_channel_snapshot(2).await.is_none());
    assert_eq!(
        controller.capture_subscriber_view().await,
        (0, false, false)
    );
    assert_eq!(controller.current_state().await, super::super::State::Idle);
    assert!(
        !bus.exists(),
        "a refused shortcut must not publish a session"
    );
}

#[tokio::test]
async fn missing_binding_does_not_open_capture() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = dir.path().join(BINDING_FILENAME);
    let shared_bus = dir.path().join("shared-bus.jsonl");
    let error = controller
        .dispatch_agent_channel(4, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect_err("missing binding must refuse");
    let message = format!("{error:#}");
    assert!(
        message.contains("missing"),
        "refusal must name the missing file: {message}"
    );
    assert!(controller.agent_channel_snapshot(4).await.is_none());
    let (count, channel, take) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0);
    assert!(!channel);
    assert!(!take);
    assert_eq!(controller.current_state().await, super::super::State::Idle);
}

#[tokio::test]
#[serial]
async fn second_press_closes_the_channel_and_leaves_dictation_idle() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
    );
    let shared_bus = dir.path().join("shared-bus.jsonl");
    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    let open = controller.agent_channel_snapshot(3).await.expect("open");
    assert_eq!(open.audience, "Leon");
    assert_eq!(open.provider.as_deref(), Some("codex"));
    assert_eq!(open.provider_session_id.as_deref(), Some("leon-session"));
    assert_eq!(
        crate::audio::streaming_recorder::channel_session_silence(open.silence_sec),
        Some(open.silence_sec),
        "the channel keeps its own hands-free silence while it stays open"
    );
    assert_eq!(controller.current_state().await, super::super::State::Idle);
    let (count, channel, take) = controller.capture_subscriber_view().await;
    assert_eq!(count, 1);
    assert!(channel);
    assert!(!take);

    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
        .await
        .expect("second press closes");
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    let (count, channel, _) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0);
    assert!(!channel);
    assert_eq!(controller.current_state().await, super::super::State::Idle);
    let rows = bus_rows(&shared_bus);
    assert_eq!(
        rows.len(),
        1,
        "a binding without a dedicated bus seals on the shared bus: {rows:?}"
    );
    assert_eq!(rows[0]["reason"], "hangup", "{rows:?}");
}

#[tokio::test]
#[serial_test::serial]
async fn conversation_refuses_while_a_channel_is_open() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude","provider_session_id":"ada-session"}}}"#,
    );
    let shared_bus = dir.path().join("shared-bus.jsonl");
    controller
        .dispatch_agent_channel(1, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    let error = controller
        .refuse_exclusive_while_agent_channel_open("conversation")
        .await
        .expect_err("exclusive surface must refuse");
    let message = format!("{error:#}");
    assert!(message.contains("agent channel"), "{message}");
    assert!(message.contains("conversation"), "{message}");
    assert!(controller.agent_channel_snapshot(1).await.is_some());
    assert_eq!(controller.current_state().await, super::super::State::Idle);
}

#[tokio::test]
#[serial]
async fn ordinary_handover_closes_each_channel_once_with_its_owned_receipt() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let first_bus = dir.path().join("channel-1.jsonl");
    let third_bus = dir.path().join("channel-3.jsonl");
    let binding = write_binding(dir.path(), &serde_json::json!({
            "schema": "vc.agent-audience-binding.v1",
            "bindings": {
                "1": {"audience": "Lena", "provider": "codex", "provider_session_id": "lena-session", "bus": first_bus},
                "3": {"audience": "Astra", "provider": "codex", "provider_session_id": "astra-session", "bus": third_bus}
            }
        }).to_string());
    let shared_bus = dir.path().join("shared.jsonl");
    for digit in [1, 3] {
        controller
            .dispatch_agent_channel(digit, &binding, &shared_bus, ChannelOpenMode::AttachedOnly)
            .await
            .expect("open fixture channel");
        let mut channels = controller.agent_channels.lock().await;
        let open = channels.get_mut(&digit).expect("open");
        open.session_id = Some(format!("agent-channel-{digit}-handover"));
        open.channel_session_id = open.session_id.clone();
    }
    let serial = controller.serial_lock.lock().await;
    controller
        .close_agent_channels_for_dictation()
        .await
        .expect("handover");
    drop(serial);
    assert!(controller.agent_channels.lock().await.is_empty());
    assert_eq!(
        controller.capture_subscriber_view().await,
        (0, false, false)
    );
    assert_eq!(controller.current_state().await, super::super::State::Idle);
    for (digit, bus, owner) in [
        (1, &first_bus, "lena-session"),
        (3, &third_bus, "astra-session"),
    ] {
        let rows = bus_rows(bus);
        assert_eq!(rows.len(), 1, "exactly one closure per owned channel");
        assert_eq!(rows[0]["reason"], "hangup");
        assert_eq!(rows[0]["state"], "sealed");
        assert_eq!(rows[0]["channel"], digit.to_string());
        assert_eq!(rows[0]["provider_session_id"], owner);
        assert_eq!(
            rows[0]["session_id"],
            format!("agent-channel-{digit}-handover")
        );
    }
    controller
        .close_agent_channels_for_dictation()
        .await
        .expect("idempotent close");
    assert_eq!(bus_rows(&first_bus).len(), 1);
    assert_eq!(bus_rows(&third_bus).len(), 1);
    assert!(
        !shared_bus.exists(),
        "no receipt is sent to an unrelated bus"
    );
}

#[tokio::test]
#[serial]
async fn ordinary_handover_refuses_failed_terminal_receipt_and_retains_owner() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    open_stamped(
        &controller,
        &binding,
        &shared_bus,
        "agent-channel-3-receipt-failure",
    )
    .await;
    let original = controller.agent_channel_snapshot(3).await.expect("owner");
    assert!(
        !channel_bus.exists(),
        "AttachedOnly fixture has no published PCM"
    );
    std::fs::create_dir_all(&channel_bus).expect("make receipt destination unwritable as a file");
    let serial = controller.serial_lock.lock().await;
    let result = controller.close_agent_channels_for_dictation().await;
    drop(serial);
    let retained = controller.agent_channel_snapshot(3).await;
    assert!(
        result.is_err(),
        "ordinary admission must refuse when its canonical hangup receipt cannot be persisted; got {result:?} with retained_owner={}",
        retained.is_some()
    );
    let retained = retained.expect("failed close retains its original owner");
    assert_eq!(retained.session_id, original.session_id);
    assert_eq!(retained.provider_session_id, original.provider_session_id);
    assert_eq!(retained.audience, original.audience);
    assert_eq!(controller.current_state().await, super::super::State::Idle);
    assert!(
        controller.session_id.read().await.is_none(),
        "no new ordinary take"
    );
    assert!(
        channel_bus.is_dir(),
        "failure fixture did not fabricate a terminal receipt"
    );
    assert!(
        !shared_bus.exists(),
        "no unrelated publication replaces the owned receipt"
    );
}

#[tokio::test]
#[serial]
async fn ordinary_handover_serializes_silence_poll_without_reopening_the_channel() {
    let controller = Arc::new(RecordingController::new_without_keychain());
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    let opened = open_stamped(
        &controller,
        &binding,
        &shared_bus,
        "agent-channel-3-handover",
    )
    .await;
    hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
    let serial = controller.serial_lock.lock().await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let polling_controller = Arc::clone(&controller);
    let poll_bus = shared_bus.clone();
    let poll = tokio::spawn(async move {
        started.send(()).expect("poll observed");
        polling_controller
            .poll_channel_autoseal(opened + Duration::from_secs(600), &poll_bus)
            .await
    });
    waiting.await.expect("poll started");
    tokio::task::yield_now().await;
    assert!(
        !poll.is_finished(),
        "silence poll waits for the existing transition owner"
    );
    controller
        .close_agent_channels_for_dictation()
        .await
        .expect("handover");
    drop(serial);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), poll)
            .await
            .expect("bounded poll")
            .expect("poll task")
            .is_empty()
    );
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    let rows = bus_rows(&channel_bus);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["reason"], "hangup");
}

#[tokio::test]
#[serial]
async fn ordinary_hold_and_toggle_admit_only_after_owned_channel_close() {
    use super::super::{HoldMode, HotkeyAction, HotkeyInput, HotkeyType, State};

    for key_type in [HotkeyType::Hold, HotkeyType::Toggle] {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let dir = tempfile::tempdir().expect("temp");
        let (binding, channel_bus) = write_dedicated_binding(dir.path());
        let shared_bus = dir.path().join("shared.jsonl");
        open_stamped(
            &controller,
            &binding,
            &shared_bus,
            "agent-channel-3-before-take",
        )
        .await;
        controller
            .handle_hotkey_event(HotkeyInput {
                key_type,
                action: if key_type == HotkeyType::Hold {
                    HotkeyAction::Down
                } else {
                    HotkeyAction::Press
                },
                assistive: false,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: false,
            })
            .await
            .expect("ordinary start");
        let expected = if key_type == HotkeyType::Hold {
            State::RecHold
        } else {
            State::RecToggle
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while controller.current_state().await != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded fake recorder admission");
        assert!(controller.agent_channel_snapshot(3).await.is_none());
        let rows = bus_rows(&channel_bus);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["reason"], "hangup");
        assert_eq!(rows[0]["session_id"], "agent-channel-3-before-take");
        let take = controller
            .session_id
            .read()
            .await
            .clone()
            .expect("ordinary take");
        assert_ne!(take, "agent-channel-3-before-take");
        // AttachedOnly stamps ownership without physical PCM. This case
        // proves admission and close ordering, not a final transcript seal.
        controller.reset().await;
        assert_eq!(controller.current_state().await, State::Idle);
    }
}

async fn admit_hold_then_chord(
    dictation: crate::config::ShortcutBinding,
    modifier: crate::config::ChannelModifier,
) -> ChordOutcome {
    use super::super::{HoldMode, HotkeyAction, HotkeyInput, HotkeyType, State};
    use crate::os::hotkeys::{
        ModeHotkeyBindings, get_channel_modifier, get_mode_hotkey_bindings, set_channel_modifier,
        set_mode_hotkey_bindings,
    };

    let previous_bindings = get_mode_hotkey_bindings();
    let previous_modifier = get_channel_modifier();
    set_mode_hotkey_bindings(ModeHotkeyBindings {
        dictation,
        ..previous_bindings
    });
    set_channel_modifier(modifier);

    let controller = Arc::new(RecordingController::new_without_keychain());
    let dir = tempfile::tempdir().expect("temp");
    let (binding, _channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    let event = |action| HotkeyInput {
        key_type: HotkeyType::Hold,
        action,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    controller
        .handle_hotkey_event(event(HotkeyAction::Down))
        .await
        .expect("arm hold");
    let admitted = tokio::time::timeout(Duration::from_secs(5), async {
        while controller.current_state().await != State::RecHold {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();

    let opened = controller
        .toggle_agent_channel_at(3, &binding, &shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("the chord opens the channel");
    let outcome = ChordOutcome {
        admitted,
        opened,
        state_after_chord: controller.current_state().await,
        session_after_chord: controller.session_id.read().await.clone(),
        channel_open: controller.agent_channel_snapshot(3).await.is_some(),
    };
    controller.reset().await;
    set_mode_hotkey_bindings(previous_bindings);
    set_channel_modifier(previous_modifier);
    outcome
}

struct ChordOutcome {
    admitted: bool,
    opened: bool,
    state_after_chord: super::super::State,
    session_after_chord: Option<String>,
    channel_open: bool,
}

#[tokio::test]
#[serial]
async fn a_hold_on_another_key_keeps_running_beside_the_opened_channel() {
    use super::super::State;
    use crate::config::{ChannelModifier, ShortcutBinding};

    let outcome = admit_hold_then_chord(ShortcutBinding::HoldFn, ChannelModifier::Ctrl).await;

    assert!(outcome.admitted);
    assert!(outcome.opened);
    assert_eq!(outcome.state_after_chord, State::RecHold);
    assert!(outcome.session_after_chord.is_some());
    assert!(outcome.channel_open);
}

#[tokio::test]
#[serial]
async fn hold_release_while_scheduling_waits_preserves_channel_and_cancels_take() {
    use super::super::{HoldMode, HotkeyAction, HotkeyInput, HotkeyType, State};

    let controller = Arc::new(RecordingController::new_without_keychain());
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    open_stamped(&controller, &binding, &shared_bus, "agent-channel-3-cancel").await;
    let original = controller
        .agent_channel_snapshot(3)
        .await
        .expect("original owner");
    let serial = controller.serial_lock.lock().await;
    let pending_controller = Arc::clone(&controller);
    let event = |action| HotkeyInput {
        key_type: HotkeyType::Hold,
        action,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    // Enter the real scheduling boundary directly so unrelated context
    // archival awaits cannot turn this into an earlier release test.
    let requested_generation = controller.hold_start_generation.load(Ordering::SeqCst);
    let pending = tokio::spawn(async move {
        pending_controller
            .schedule_hold_start(false, requested_generation)
            .await
    });
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        !pending.is_finished(),
        "start waits for handover serialization"
    );
    controller
        .handle_hotkey_event(event(HotkeyAction::Up))
        .await
        .expect("release does not wait for drain");
    drop(serial);
    tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .expect("bounded handover")
        .expect("task")
        .expect("start resolves");
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(controller.session_id.read().await.is_none());
    assert!(controller.hold_start_task.lock().await.is_none());
    let retained = controller
        .agent_channel_snapshot(3)
        .await
        .expect("retained owner");
    assert_eq!(retained.session_id, original.session_id);
    assert_eq!(
        retained.session_id.as_deref(),
        Some("agent-channel-3-cancel")
    );
    assert_eq!(retained.provider_session_id, original.provider_session_id);
    assert_eq!(retained.audience, original.audience);
    assert!(
        bus_rows(&channel_bus).is_empty(),
        "cancelled admission cannot hang up the channel"
    );
}

#[tokio::test]
#[serial]
async fn raw_hold_arm_preserves_channel_until_an_ordinary_take_is_admitted() {
    use super::super::{HoldMode, HotkeyAction, HotkeyInput, HotkeyType, State};
    let controller = Arc::new(RecordingController::new_without_keychain());
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared.jsonl");
    open_stamped(
        &controller,
        &binding,
        &shared_bus,
        "agent-channel-3-before-chord",
    )
    .await;
    let event = |action| HotkeyInput {
        key_type: HotkeyType::Hold,
        action,
        assistive: false,
        hold_mode: HoldMode::Raw,
        force_raw: false,
        force_ai: false,
    };
    controller
        .handle_hotkey_event(event(HotkeyAction::Down))
        .await
        .expect("arm hold");
    let channel_preserved = controller.agent_channel_snapshot(3).await.is_some();
    controller
        .handle_hotkey_event(event(HotkeyAction::Up))
        .await
        .expect("cancel arm");
    assert_eq!(controller.current_state().await, State::Idle);
    assert!(controller.session_id.read().await.is_none());
    assert!(
        channel_preserved,
        "a hold arm inside the start delay cannot hang up a channel before dictation admission"
    );
    assert!(
        bus_rows(&channel_bus).is_empty(),
        "no premature hangup receipt"
    );
}

#[test]
fn channel_timing_reads_the_send_pause_and_zero_disables() {
    use codescribe_core::config::agent_channel::{AgentChannelConfig, ChannelSettingSource};
    let default = ChannelTiming::default();
    assert_eq!(default.autoseal_secs(), 5);
    assert!(default.continuous());
    assert_eq!(
        default.voiceless_close, None,
        "a conversation never ends for lack of voice"
    );
    let env = |seconds: f32| {
        ChannelTiming::from_config(AgentChannelConfig {
            send_pause_sec: seconds,
            send_pause_source: ChannelSettingSource::EnvOverride,
            voiceless_close_sec: 30,
            ..AgentChannelConfig::default()
        })
    };
    assert_eq!(env(15.0).autoseal_secs(), 15);
    assert_eq!(env(4.5).autoseal_secs(), 5, "rounded up for the row");
    assert_eq!(env(4.5).utterance_silence_sec(), 4.5);
    assert_eq!(env(0.0).autoseal_secs(), 0);
    assert_eq!(env(0.0).send_pause, None);
    assert_eq!(env(0.0).voiceless_close, None, "0 turns both off");
    assert_eq!(
        env(40.0).autoseal_secs(),
        40,
        "the env number is kept as given"
    );
}

/// The pre-continuous seal contract: one message per press, with the
/// former fixed voiceless close. `secs = 0` turns both off.
fn one_message_timing(secs: u64) -> ChannelTiming {
    let send_pause = (secs > 0).then(|| Duration::from_secs(secs));
    ChannelTiming {
        policy: ChannelSessionPolicy::EndAfterMessage,
        send_pause,
        voiceless_close: send_pause.map(|_| Duration::from_secs(VOICELESS_CLOSE_SECS)),
        ..ChannelTiming::default()
    }
}

async fn seal_with(
    controller: &RecordingController,
    now: SystemTime,
    bus: &Path,
    secs: u64,
) -> Vec<u8> {
    {
        let mut channels = controller.agent_channels.lock().await;
        for open in channels.values_mut() {
            open.timing = one_message_timing(secs);
        }
    }
    controller.seal_channels_silent_for(now, bus).await
}

#[tokio::test]
#[serial]
async fn silence_past_the_cap_seals_the_channel_and_clears_the_open_state() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
    );
    let bus = dir.path().join("bus.jsonl");
    controller
        .dispatch_agent_channel(3, &path, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    let hud = controller.channel_hud_states().await;
    assert_eq!(hud.len(), 1);
    assert!(hud[0].open && hud[0].loud, "{hud:?}");
    assert_eq!(hud[0].channel, "3");
    assert_eq!(hud[0].audience, "Leon");
    assert!(hud[0].label.contains("CHANNEL 3 OPEN"), "{}", hud[0].label);
    assert!(!hud[0].tts_ducking);
    assert_eq!(hud[0].provider.as_deref(), Some("codex"));
    assert_eq!(hud[0].provider_session_id.as_deref(), Some("leon-session"));

    let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
    let opened = snapshot.opened_at;
    let still_open = seal_with(&controller, opened + Duration::from_secs(10), &bus, 120).await;
    assert!(still_open.is_empty());
    assert!(controller.agent_channel_snapshot(3).await.is_some());
    let disabled = seal_with(&controller, opened + Duration::from_secs(10_000), &bus, 0).await;
    assert!(disabled.is_empty(), "0 turns automatic closing off");
    assert!(controller.channel_hud_states().await[0].loud);

    measured_speech_end(&snapshot, opened + Duration::from_secs(60));
    let too_soon = seal_with(&controller, opened + Duration::from_secs(179), &bus, 120).await;
    assert!(
        too_soon.is_empty(),
        "silence is measured from the last voice"
    );
    let sealed = seal_with(&controller, opened + Duration::from_secs(180), &bus, 120).await;
    assert_eq!(sealed, vec![3]);
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert!(controller.channel_hud_states().await.is_empty());
    let (count, channel, _) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0);
    assert!(!channel);
    let again = seal_with(&controller, opened + Duration::from_secs(500), &bus, 120).await;
    assert!(again.is_empty());

    let text = std::fs::read_to_string(&bus).expect("sealed receipt");
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(rows.len(), 1, "{text}");
    let row = &rows[0];
    assert_eq!(
        row.get("schema").and_then(|value| value.as_str()),
        Some("codescribe.channel-session.v1")
    );
    assert_eq!(
        row.get("kind").and_then(|value| value.as_str()),
        Some("channel_session")
    );
    assert_eq!(
        row.get("state").and_then(|value| value.as_str()),
        Some("sealed")
    );
    assert_eq!(
        row.get("reason").and_then(|value| value.as_str()),
        Some("silence")
    );
    assert_eq!(
        row.get("channel").and_then(|value| value.as_str()),
        Some("3")
    );
    assert_eq!(
        row.get("agent").and_then(|value| value.as_str()),
        Some("Leon")
    );
    assert_eq!(
        row.get("provider").and_then(|value| value.as_str()),
        Some("codex")
    );
    assert_eq!(
        row.get("provider_session_id")
            .and_then(|value| value.as_str()),
        Some("leon-session")
    );
    assert!(
        row.get("opened_at")
            .and_then(|value| value.as_str())
            .is_some_and(|stamp| stamp.ends_with('Z')),
        "{row}"
    );
    assert_eq!(
        row.get("loud").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        row.get("autoseal_secs").and_then(|value| value.as_u64()),
        Some(120)
    );
    let emitted_at = row
        .get("emitted_at")
        .and_then(|value| value.as_str())
        .unwrap();
    assert!(emitted_at.ends_with('Z'), "{emitted_at}");
}

#[tokio::test]
#[serial]
async fn a_binding_bus_routes_the_channel_receipts_to_the_dedicated_file() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());

    let bound = resolve_digit(3, &path).expect("bound session");
    assert_eq!(bound.bus.as_deref(), Some(channel_bus.as_path()));

    let shared_bus = dir.path().join("shared-bus.jsonl");
    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
    assert_eq!(snapshot.bus.as_deref(), Some(channel_bus.as_path()));
    let opened = snapshot.opened_at;
    measured_speech_end(&snapshot, opened + Duration::from_secs(60));

    let sealed = seal_with(
        &controller,
        opened + Duration::from_secs(180),
        &shared_bus,
        120,
    )
    .await;
    assert_eq!(sealed, vec![3]);

    assert!(
        !shared_bus.exists(),
        "a dedicated-bus channel writes nothing to the shared bus"
    );
    let text = std::fs::read_to_string(&channel_bus).expect("dedicated receipt");
    let row: serde_json::Value =
        serde_json::from_str(text.lines().next().expect("one row")).expect("json");
    assert_eq!(
        row.get("state").and_then(|value| value.as_str()),
        Some("sealed")
    );
    assert_eq!(
        row.get("channel").and_then(|value| value.as_str()),
        Some("3")
    );
}

/// One press, one message (Founder decision 2026-10-26; replaces the
/// reopen half of voice seal cc6c8248): silence delivers the utterance and
/// ends the session like a hang-up. Nothing reopens it and the capture is
/// released, so the microphone goes off with the delivery.
#[tokio::test]
#[serial]
async fn silence_seal_ends_the_session_and_releases_the_capture() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let path = write_binding(
        dir.path(),
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
    );
    let bus = dir.path().join("bus.jsonl");
    controller
        .dispatch_agent_channel(3, &path, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    let snapshot = controller.agent_channel_snapshot(3).await.expect("open");
    let opened = snapshot.opened_at;
    measured_speech_end(&snapshot, opened + Duration::from_secs(1));

    let sealed = seal_with(&controller, opened + Duration::from_secs(6), &bus, 5).await;
    assert_eq!(sealed, vec![3]);

    assert!(
        controller.agent_channel_snapshot(3).await.is_none(),
        "nothing reopens the channel after delivery"
    );
    assert!(controller.channel_hud_states().await.is_empty());
    let (count, channel, _) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0, "the channel capture is released");
    assert!(!channel);
    let later = seal_with(&controller, opened + Duration::from_secs(600), &bus, 5).await;
    assert!(later.is_empty(), "no session is left to seal");

    let rows = bus_rows(&bus);
    assert_eq!(rows.len(), 1, "one silence seal, no new open: {rows:?}");
    assert_eq!(rows[0]["state"], "sealed");
    assert_eq!(rows[0]["reason"], "silence");
}

/// A press that brings no voice does not keep the microphone live
/// (Founder decision 2026-10-26): the session ends through the silence
/// seal once the voiceless limit has passed since it opened. Heard voice
/// moves the session to the utterance clock instead.
#[tokio::test]
#[serial]
async fn a_channel_that_hears_no_voice_ends_after_the_voiceless_limit() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    let limit = Duration::from_secs(VOICELESS_CLOSE_SECS);

    let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-silent").await;
    let too_soon = seal_with(
        &controller,
        opened + limit - Duration::from_secs(1),
        &shared_bus,
        5,
    )
    .await;
    assert!(too_soon.is_empty(), "the limit counts from the open");
    let disabled = seal_with(&controller, opened + limit * 10, &shared_bus, 0).await;
    assert!(disabled.is_empty(), "0 turns automatic closing off");
    let sealed = seal_with(&controller, opened + limit, &shared_bus, 5).await;
    assert_eq!(sealed, vec![3]);
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert!(controller.channel_hud_states().await.is_empty());
    let (count, channel, _) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0, "the channel capture is released");
    assert!(!channel);

    let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-spoken").await;
    hear_voice_at(&controller, opened + limit).await;
    let heard = seal_with(
        &controller,
        opened + limit + Duration::from_secs(2),
        &shared_bus,
        5,
    )
    .await;
    assert!(
        heard.is_empty(),
        "heard voice runs on the utterance silence"
    );
    assert!(controller.agent_channel_snapshot(3).await.is_some());
    let rows = sealed_rows(&bus_rows(&channel_bus));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "agent-channel-3-silent");
    assert_eq!(rows[0].1, "silence");
    assert!(!shared_bus.exists());
}

/// The observed loss: drafts reached the bus, the ledger refused terminal
/// finality, and the Founder hung up before the silence boundary. The
/// hang-up itself must put the closing row on the channel's own bus.
#[tokio::test]
#[serial]
async fn hang_up_before_any_seal_writes_one_hangup_receipt_on_the_channel_bus() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    let session = "agent-channel-3-hangup";
    let opened = open_stamped(&controller, &path, &shared_bus, session).await;
    // Voice was heard one second in; the five-second silence boundary
    // has not passed, so no silence seal can precede the hang-up.
    hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
    assert!(
        seal_with(&controller, opened + Duration::from_secs(2), &shared_bus, 5)
            .await
            .is_empty()
    );

    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
        .await
        .expect("second press hangs up");

    assert!(controller.agent_channel_snapshot(3).await.is_none());
    let (count, channel, _) = controller.capture_subscriber_view().await;
    assert_eq!(count, 0);
    assert!(!channel);
    assert!(
        !shared_bus.exists(),
        "a dedicated-bus channel writes nothing to the shared bus"
    );
    let rows = bus_rows(&channel_bus);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["schema"], "codescribe.channel-session.v1");
    assert_eq!(row["kind"], "channel_session");
    assert_eq!(row["state"], "sealed");
    assert_eq!(row["reason"], "hangup");
    assert_eq!(row["channel"], "3");
    assert_eq!(row["agent"], "Leon");
    assert_eq!(row["session_id"], session);
    assert_eq!(row["loud"], false);
    assert_eq!(row["provider"], "codex");
    assert_eq!(row["provider_session_id"], "leon-session");
    assert_eq!(
        row["autoseal_secs"],
        ChannelTiming::default().autoseal_secs()
    );
    assert!(
        row["opened_at"]
            .as_str()
            .is_some_and(|stamp| stamp.ends_with('Z')),
        "{row}"
    );
    assert!(
        row["emitted_at"]
            .as_str()
            .is_some_and(|stamp| stamp.ends_with('Z')),
        "{row}"
    );

    // The hung-up session is gone: a later silence poll seals nothing.
    let later = seal_with(
        &controller,
        opened + Duration::from_secs(600),
        &shared_bus,
        5,
    )
    .await;
    assert!(later.is_empty());
    assert_eq!(bus_rows(&channel_bus).len(), 1);
}

/// The roster names every bound digit and says who is actually listening:
/// a fresh lease heartbeat marks the follower alive, a missing lease marks
/// it dead, and a bound-but-closed digit still appears with `open: false`.
#[tokio::test]
async fn a_roster_row_carries_follower_liveness_for_bound_closed_channels() {
    let dir = tempfile::tempdir().expect("temp");
    let binding = dir.path().join("binding.json");
    std::fs::write(
            &binding,
            r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"1":{"audience":"Ada","provider":"claude-code","provider_session_id":"ada-session"},"3":{"audience":"Leon","provider":"codex","provider_session_id":"leon-session"}}}"#,
        )
        .expect("binding");
    let bridge = dir.path().join("bridge");
    std::fs::create_dir_all(bridge.join("leases")).expect("leases dir");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_secs_f64();
    std::fs::write(
            bridge.join("leases/ada.json"),
            format!(
                r#"{{"schema":"codescribe.agent-bridge.lease.v1","lease_id":"ada","name":"ada","active":true,"heartbeat_unix":{now},"provider":"claude-code","provider_session_id":"ada-session","pending":[{{"kind":"seal","delivery_id":"aaaaaaaaaaaaaaaaaaaaaaaa","emitted_at":"2026-10-14T12:00:00Z"}},{{"kind":"seal","delivery_id":"bbbbbbbbbbbbbbbbbbbbbbbb","emitted_at":"2026-10-14T11:00:00Z"}}]}}"#,
            ),
        )
        .expect("lease");

    let controller = RecordingController::new_without_keychain();
    let roster = controller.channel_roster_states_at(&binding, &bridge).await;

    assert_eq!(roster.len(), 3, "{roster:?}");
    assert_eq!(roster[0].channel, "0");
    assert_eq!(roster[0].audience, "*");
    assert!(roster[0].provider.is_none());
    assert!(roster[0].provider_session_id.is_none());
    assert_eq!(roster[1].channel, "1");
    assert_eq!(roster[1].audience, "Ada");
    assert!(!roster[1].open);
    assert_eq!(roster[1].follower_alive, Some(true));
    assert_eq!(
        roster[1].unread_since,
        Some(
            SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    chrono::DateTime::parse_from_rfc3339("2026-10-14T11:00:00Z")
                        .expect("stamp")
                        .timestamp() as u64
                )
        ),
        "the oldest unacknowledged delivery dates the backlog of a live follower"
    );
    assert_eq!(roster[2].channel, "3");
    assert_eq!(roster[2].audience, "Leon");
    assert!(!roster[2].open);
    assert_eq!(
        roster[2].follower_alive,
        Some(false),
        "no lease means nobody is listening"
    );
    assert_eq!(
        roster[2].unread_since, None,
        "a dead follower has no live backlog"
    );
}

/// A Codex app follower is usually dead between turns; its row still
/// carries the thread link the app opens itself, built here once.
#[tokio::test]
async fn a_roster_row_carries_the_codex_thread_of_a_dead_follower() {
    const THREAD: &str = "c0ffee01-0000-7000-8000-000000000001";
    let dir = tempfile::tempdir().expect("temp");
    let binding = dir.path().join("binding.json");
    std::fs::write(
            &binding,
            format!(
                r#"{{"schema":"vc.agent-audience-binding.v1","bindings":{{"1":{{"audience":"Ada","provider":"claude-code","provider_session_id":"ada-session"}},"3":{{"audience":"Leon","provider":"codex","provider_session_id":"{THREAD}"}}}}}}"#,
            ),
        )
        .expect("binding");
    let bridge = dir.path().join("bridge");
    std::fs::create_dir_all(bridge.join("leases")).expect("leases dir");
    std::fs::write(
            bridge.join("leases/leon.json"),
            format!(
                r#"{{"schema":"codescribe.agent-bridge.lease.v1","lease_id":"leon","name":"leon","active":false,"heartbeat_unix":1000.0,"provider":"codex","provider_session_id":"{THREAD}","codex":{{"thread_id":"{THREAD}","app_bundle_id":"com.openai.codex","rollout":true,"remote":false}}}}"#,
            ),
        )
        .expect("lease");

    let controller = RecordingController::new_without_keychain();
    let roster = controller.channel_roster_states_at(&binding, &bridge).await;

    assert_eq!(roster.len(), 3, "{roster:?}");
    assert_eq!(roster[2].channel, "3");
    assert_eq!(roster[2].follower_alive, Some(false));
    assert_eq!(
        roster[2].codex_thread_url.as_deref(),
        Some(format!("codex://threads/{THREAD}").as_str())
    );
    assert_eq!(roster[2].codex_thread_local, Some(true));
    assert_eq!(roster[1].codex_thread_url, None, "no lease, no thread");
    assert_eq!(roster[1].codex_thread_local, None);
    assert_eq!(roster[0].codex_thread_url, None);
}

#[test]
fn frozen_broadcast_admits_heartbeats_refreshed_during_large_lease_read() {
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().expect("temp");
    let bus = dir.path().join("bus.jsonl");
    std::fs::write(&bus, "").expect("bus");
    let bus = bus.canonicalize().expect("canonical bus");
    let leases = dir.path().join("leases");
    std::fs::create_dir(&leases).expect("leases");
    let heartbeat = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_secs_f64()
        - 1.0;
    let mut bindings = serde_json::Map::new();
    let mut records = Vec::new();
    for digit in 1..=4 {
        let session = format!("recipient-clock-{digit}");
        let lease =
            hex::encode(Sha256::digest(format!("codex\0{session}").as_bytes()))[..32].to_string();
        bindings.insert(
            digit.to_string(),
            serde_json::json!({
                "audience": format!("agent-{digit}"), "provider": "codex",
                "provider_session_id": session, "bus": bus
            }),
        );
        records.push((
            leases.join(format!("{lease}.json")),
            serde_json::json!({
                "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
                "provider": "codex", "provider_session_id": session, "active": true,
                "heartbeat_unix": heartbeat, "bus": bus
            }),
        ));
    }
    let binding = write_binding(
        dir.path(),
        &serde_json::json!({
            "schema": BINDING_SCHEMA, "bindings": bindings
        })
        .to_string(),
    );
    let (first_path, mut first) = records.remove(0);
    first["diagnostic_padding"] = serde_json::Value::String("x".repeat(2_200_000));
    let bytes = serde_json::to_vec(&first).expect("large lease");
    assert!(bytes.len() > 2 << 20);
    for (path, record) in &records {
        std::fs::write(path, serde_json::to_vec(record).expect("json")).expect("lease");
    }
    // A fixture-only FIFO gates the real File read without clock or parser
    // substitutions. Its writer opens only once the function has reached
    // the first lease, then refreshes later owners before supplying bytes.
    let name = std::ffi::CString::new(first_path.as_os_str().as_bytes()).expect("path");
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let writer = std::thread::spawn(move || {
        let mut first_file = std::fs::OpenOptions::new()
            .write(true)
            .open(first_path)
            .expect("gated lease");
        let refreshed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_secs_f64();
        for (path, mut record) in records {
            record["heartbeat_unix"] = serde_json::json!(refreshed);
            std::fs::write(path, serde_json::to_vec(&record).expect("json")).expect("refresh");
        }
        first_file.write_all(&bytes).expect("large lease bytes");
    });
    let started = std::time::Instant::now();
    let recipients = frozen_channel_recipients(0, &binding, &bus);
    writer.join().expect("writer");
    eprintln!(
        "large-lease recipient selection: {:?}, {} owners",
        started.elapsed(),
        recipients.len()
    );
    assert_eq!(
        recipients.len(),
        4,
        "Refresh during I/O must not look like a future heartbeat"
    );
    assert_eq!(
        recipients
            .iter()
            .map(|row| row["channel"].as_str().expect("channel"))
            .collect::<Vec<_>>(),
        vec!["1", "2", "3", "4"]
    );
}

#[test]
fn frozen_broadcast_rejects_stale_future_and_foreign_lease_metadata() {
    let dir = tempfile::tempdir().expect("temp");
    let bus = dir.path().join("bus.jsonl");
    std::fs::write(&bus, "").expect("bus");
    let bus = bus.canonicalize().expect("canonical bus");
    std::fs::create_dir(dir.path().join("leases")).expect("leases");
    let session = "recipient-clock-controls";
    let lease =
        hex::encode(Sha256::digest(format!("codex\0{session}").as_bytes()))[..32].to_string();
    let path = dir.path().join("leases").join(format!("{lease}.json"));
    let heartbeat = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_secs_f64();
    let owned = serde_json::json!({
        "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
        "provider": "codex", "provider_session_id": session, "active": true,
        "heartbeat_unix": heartbeat, "bus": bus
    });
    let binding = write_binding(
        dir.path(),
        &serde_json::json!({
            "schema": BINDING_SCHEMA, "bindings": {
                "1": {"audience": "owner", "provider": "codex", "provider_session_id": session}
            }
        })
        .to_string(),
    );
    std::fs::write(&path, serde_json::to_vec(&owned).expect("json")).expect("lease");
    assert_eq!(frozen_channel_recipients(0, &binding, &bus).len(), 1);
    assert_eq!(frozen_channel_recipients(1, &binding, &bus).len(), 1);
    assert!(frozen_channel_recipients(2, &binding, &bus).is_empty());
    for (field, value) in [
        (
            "heartbeat_unix",
            serde_json::json!(heartbeat - active_names::LEASE_TTL_SECONDS - 10.0),
        ),
        ("heartbeat_unix", serde_json::json!(heartbeat + 30.0)),
        ("heartbeat_unix", serde_json::Value::Null),
        ("active", serde_json::json!(false)),
        ("schema", serde_json::json!("wrong-schema")),
        ("provider", serde_json::json!("claude-code")),
        ("provider_session_id", serde_json::json!("another-session")),
        ("lease_id", serde_json::json!("another-lease")),
        ("bus", serde_json::json!("/tmp/not-owned.jsonl")),
    ] {
        let mut rejected = owned.clone();
        rejected[field] = value.clone();
        std::fs::write(&path, serde_json::to_vec(&rejected).expect("json")).expect("lease");
        assert!(
            frozen_channel_recipients(0, &binding, &bus).is_empty(),
            "{field}={value}"
        );
        assert!(
            frozen_channel_recipients(1, &binding, &bus).is_empty(),
            "direct {field}={value}"
        );
    }
    for bytes in [b"{".to_vec(), vec![b' '; (16 << 20) + 1]] {
        std::fs::write(&path, bytes).expect("invalid lease");
        assert!(frozen_channel_recipients(0, &binding, &bus).is_empty());
    }
}

/// Maciej unticked channel 0 for John at 08:06:58Z on 2026-10-18; his
/// next channel-0 take was still written to John's bus and overwrote his
/// row preview. An owner with `policy: "off"` leaves the broadcast only.
#[test]
fn frozen_broadcast_skips_owner_who_unticked_channel_zero() {
    let dir = tempfile::tempdir().expect("temp");
    let bus = dir.path().join("bus.jsonl");
    std::fs::write(&bus, "").expect("bus");
    let bus = bus.canonicalize().expect("canonical bus");
    std::fs::create_dir(dir.path().join("leases")).expect("leases");
    let session = "unticked-channel-zero";
    let lease =
        hex::encode(Sha256::digest(format!("codex\0{session}").as_bytes()))[..32].to_string();
    let heartbeat = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_secs_f64();
    std::fs::write(
        dir.path().join("leases").join(format!("{lease}.json")),
        serde_json::to_vec(&serde_json::json!({
            "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
            "provider": "codex", "provider_session_id": session, "active": true,
            "heartbeat_unix": heartbeat, "bus": bus
        }))
        .expect("json"),
    )
    .expect("lease");
    let binding = write_binding(
        dir.path(),
        &serde_json::json!({
            "schema": BINDING_SCHEMA, "bindings": {
                "6": {"audience": "john", "provider": "codex", "provider_session_id": session}
            }
        })
        .to_string(),
    );
    let key = hex::encode(Sha256::digest(
        format!("codex\0{session}\0{}", bus.display()).as_bytes(),
    ))[..24]
        .to_string();
    let policies = dir.path().join("runtime").join("broadcast-policy");
    std::fs::create_dir_all(&policies).expect("policies");
    let receipt = serde_json::json!({
        "schema": "codescribe.broadcast-policy.v1", "provider": "codex",
        "provider_session_id": session, "lease_id": lease, "bus": bus,
        "origin_channel": "0", "policy": "off"
    });
    let path = policies.join(format!("{key}.json"));
    std::fs::write(&path, serde_json::to_vec(&receipt).expect("json")).expect("policy");
    assert!(frozen_channel_recipients(0, &binding, &bus).is_empty());
    assert_eq!(
        frozen_channel_recipients(6, &binding, &bus).len(),
        1,
        "leaving channel 0 never blocks words addressed to the agent"
    );
    for (field, value) in [
        ("policy", serde_json::json!("quiet")),
        ("policy", serde_json::json!("on")),
        ("schema", serde_json::json!("wrong-schema")),
        ("lease_id", serde_json::json!("another-lease")),
        ("bus", serde_json::json!("/tmp/not-owned.jsonl")),
    ] {
        let mut kept = receipt.clone();
        kept[field] = value.clone();
        std::fs::write(&path, serde_json::to_vec(&kept).expect("json")).expect("policy");
        assert_eq!(
            frozen_channel_recipients(0, &binding, &bus).len(),
            1,
            "{field}={value}"
        );
    }
    std::fs::write(&path, b"{").expect("broken policy");
    assert_eq!(
        frozen_channel_recipients(0, &binding, &bus).len(),
        1,
        "a broken receipt fails open, as the helper does"
    );
}

#[test]
fn frozen_broadcast_keeps_original_owners_and_refuses_wrong_destinations() {
    let dir = tempfile::tempdir().expect("temp");
    let binding = dir.path().join("binding.json");
    let bus = dir.path().join("bus.jsonl");
    std::fs::write(&bus, "").expect("bus");
    let canonical_bus = bus.canonicalize().expect("canonical bus");
    std::fs::create_dir(dir.path().join("leases")).expect("leases");
    let bindings = serde_json::json!({"schema": "vc.agent-audience-binding.v1", "bindings": {
        "1": {"audience": "Lena", "provider": "codex", "provider_session_id": "agent-a"},
        "2": {"audience": "Adam", "provider": "codex", "provider_session_id": "agent-b"},
        "3": {"audience": "Astra", "provider": "codex", "provider_session_id": "agent-c"}
    }});
    std::fs::write(&binding, serde_json::to_vec(&bindings).expect("json")).expect("binding");
    let admit = |session: &str, destination: &Path| {
        let lease =
            hex::encode(Sha256::digest(format!("codex\0{session}").as_bytes()))[..32].to_string();
        let receipt = serde_json::json!({
            "schema": "codescribe.agent-bridge.lease.v1", "lease_id": lease,
            "provider": "codex", "provider_session_id": session, "active": true,
            "heartbeat_unix": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("clock").as_secs_f64(),
            "bus": destination
        });
        std::fs::write(
            dir.path().join("leases").join(format!("{lease}.json")),
            serde_json::to_vec(&receipt).expect("json"),
        )
        .expect("lease");
    };
    admit("agent-a", &canonical_bus);
    let frozen = frozen_channel_recipients(0, &binding, &bus);
    assert_eq!(frozen.len(), 1);
    admit("agent-b", &canonical_bus);
    admit("agent-c", &dir.path().join("wrong.jsonl"));
    assert_eq!(
        frozen.len(),
        1,
        "a late attachment cannot inherit this question"
    );
    assert_eq!(frozen[0]["provider_session_id"], "agent-a");
    let next = frozen_channel_recipients(0, &binding, &bus);
    assert_eq!(
        next.len(),
        2,
        "only concrete live owners of this destination"
    );
    let named = frozen_channel_recipients(2, &binding, &bus);
    assert_eq!(named.len(), 1);
    assert_eq!(named[0]["provider_session_id"], "agent-b");
    assert!(frozen_channel_recipients(3, &binding, &bus).is_empty());
}

/// A press made to answer a long agent reply stays open through the
/// reply: the channel hears nothing while it plays, so that time is not
/// voiceless, and the limit counts again from the end of the playback.
#[tokio::test]
#[serial]
async fn agent_playback_does_not_count_toward_the_voiceless_limit() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    let limit = Duration::from_secs(VOICELESS_CLOSE_SECS);
    let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-answer").await;

    crate::audio::tts_duck::arm_for(Duration::from_secs(600));
    let playing = opened + limit + Duration::from_secs(10);
    let during = seal_with(&controller, playing, &shared_bus, 5).await;
    crate::audio::tts_duck::clear();
    assert!(
        during.is_empty(),
        "a reply playing into the channel is not silence"
    );
    let after = seal_with(
        &controller,
        playing + limit - Duration::from_secs(1),
        &shared_bus,
        5,
    )
    .await;
    assert!(
        after.is_empty(),
        "the limit counts again from the end of the playback"
    );
    let sealed = seal_with(&controller, playing + limit, &shared_bus, 5).await;
    assert_eq!(sealed, vec![3]);
    assert_eq!(sealed_rows(&bus_rows(&channel_bus)).len(), 1);
}

/// One closing throne: a session sealed by silence is never sealed again
/// by a hang-up. Silence ends the session, so the next press opens a new
/// one, and the press after it hangs that one up.
#[tokio::test]
#[serial]
async fn a_silence_sealed_session_is_never_sealed_again_by_a_hang_up() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");

    let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-first").await;
    hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
    let sealed = seal_with(&controller, opened + Duration::from_secs(6), &shared_bus, 5).await;
    assert_eq!(sealed, vec![3]);
    assert!(controller.agent_channel_snapshot(3).await.is_none());

    open_stamped(&controller, &path, &shared_bus, "agent-channel-3-next").await;
    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
        .await
        .expect("hang up the next session");
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert_eq!(
        sealed_rows(&bus_rows(&channel_bus)),
        vec![
            ("agent-channel-3-first".to_string(), "silence".to_string()),
            ("agent-channel-3-next".to_string(), "hangup".to_string()),
        ]
    );
    assert!(!shared_bus.exists());
}

/// A hang-up ends one session, not the binding: the next press opens on
/// the same binding, and silence after voice ends that session too.
#[tokio::test]
#[serial]
async fn a_press_after_a_hang_up_opens_and_silence_ends_it() {
    crate::audio::tts_duck::clear();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (path, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");

    open_stamped(&controller, &path, &shared_bus, "agent-channel-3-before").await;
    controller
        .dispatch_agent_channel(3, &path, &shared_bus, ChannelOpenMode::Live)
        .await
        .expect("hang up");
    assert!(controller.channel_hud_states().await.is_empty());

    let opened = open_stamped(&controller, &path, &shared_bus, "agent-channel-3-after").await;
    let hud = controller.channel_hud_states().await;
    assert_eq!(hud.len(), 1);
    assert!(hud[0].open && hud[0].loud, "{hud:?}");
    assert_eq!(hud[0].audience, "Leon");
    hear_voice_at(&controller, opened + Duration::from_secs(1)).await;
    let sealed = seal_with(&controller, opened + Duration::from_secs(6), &shared_bus, 5).await;
    assert_eq!(sealed, vec![3]);
    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert!(controller.channel_hud_states().await.is_empty());
    assert_eq!(
        sealed_rows(&bus_rows(&channel_bus)),
        vec![
            ("agent-channel-3-before".to_string(), "hangup".to_string()),
            ("agent-channel-3-after".to_string(), "silence".to_string()),
        ]
    );
}

/// Byte shape of the `open` row the writer put on the shared bus for a
/// channel whose app was then quit (observed 2026-09-29, channel 2).
/// Identities are neutral; field order and formats are the writer's.
const ORPHAN_SESSION: &str = "agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f";
const ORPHAN_OPEN_ROW: &str = r#"{"agent":"Leon","autoseal_secs":5,"channel":"2","emitted_at":"2026-09-29T17:22:06.346400Z","kind":"channel_session","loud":true,"opened_at":"2026-09-29T17:22:06.265163Z","provider":"claude-code","provider_session_id":"leon-session","reason":"opened","schema":"codescribe.channel-session.v1","session_id":"agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f","state":"open","utterance_silence_sec":5.0}"#;
/// The session's ledger terminal seal, which followed its `open` row.
/// It is transcript evidence, not a channel-session boundary.
const ORPHAN_TERMINAL_SEAL_ROW: &str = r#"{"schema":"codescribe.transcript-evidence.v1","sequence":3,"session_id":"agent-channel-2-5f0c1d2e-20df-4f28-954f-0a1b2c3d4e5f","reducer_revision":5,"reducer_action":"record_ledger_terminal_seal","document_index":0,"rendered_text":"Iwo","terminal":false,"audience":"Leon"}"#;
/// An earlier capture of the same channel, still draining when its successor opened.
const SUPERSEDED_OPEN_ROW: &str = r#"{"agent":"Leon","autoseal_secs":5,"channel":"2","emitted_at":"2026-09-29T16:34:23.279789Z","kind":"channel_session","loud":true,"opened_at":"2026-09-29T16:34:23.199613Z","provider":"claude-code","provider_session_id":"leon-session","reason":"opened","schema":"codescribe.channel-session.v1","session_id":"agent-channel-2-superseded","state":"open","utterance_silence_sec":5.0}"#;
const CHANNEL_2_BINDING: &str = r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"2":{"audience":"Leon","provider":"claude-code","provider_session_id":"leon-session"}}}"#;

/// Appends rows exactly as another process left them.
fn append_raw(bus: &Path, rows: &[&str]) {
    use std::io::Write;
    if let Some(parent) = bus.parent() {
        std::fs::create_dir_all(parent).expect("bus dir");
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(bus)
        .expect("bus");
    for row in rows {
        writeln!(file, "{row}").expect("row");
    }
}

/// The `open` row a Live dispatch writes for `session` on channel 3.
fn live_open_line(session: &str) -> crate::presentation::agent_ack::ChannelSessionLine<'_> {
    crate::presentation::agent_ack::ChannelSessionLine {
        state: "open",
        reason: "opened",
        channel: "3",
        agent: "Leon",
        session_id: Some(session),
        autoseal_secs: 5,
        opened_at: SystemTime::now(),
        utterance_silence_sec: 5.0,
        provider: Some("codex"),
        provider_session_id: Some("leon-session"),
    }
}

fn write_open_row(bus: &Path, open: &crate::presentation::agent_ack::ChannelSessionLine<'_>) {
    crate::presentation::agent_ack::append_json_line(
        bus,
        &crate::presentation::agent_ack::channel_session_line(open),
    )
    .expect("open row");
}

#[test]
fn orphan_recovery_keeps_a_draining_predecessor_when_a_successor_opened() {
    let dir = tempfile::tempdir().unwrap();
    let bus = dir.path().join("overlap.jsonl");
    append_raw(
        &bus,
        &[
            SUPERSEDED_OPEN_ROW,
            ORPHAN_OPEN_ROW,
            ORPHAN_TERMINAL_SEAL_ROW,
        ],
    );
    let recovered = seal_orphaned_sessions(
        std::slice::from_ref(&bus),
        &BTreeSet::new(),
        ORPHAN_SCAN_WINDOW_BYTES,
    );
    let sessions = recovered
        .iter()
        .map(|row| row.session_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        sessions,
        BTreeSet::from(["agent-channel-2-superseded", ORPHAN_SESSION]),
        "a replacement capture never closes its predecessor's retained decoder tail"
    );
    assert!(
        seal_orphaned_sessions(
            std::slice::from_ref(&bus),
            &BTreeSet::new(),
            ORPHAN_SCAN_WINDOW_BYTES
        )
        .is_empty(),
        "named recovery closes each capture once"
    );
}

#[test]
fn orphan_recovery_close_names_only_its_capture_and_preserves_live_successor() {
    let dir = tempfile::tempdir().unwrap();
    let bus = dir.path().join("overlap-closed.jsonl");
    append_raw(&bus, &[SUPERSEDED_OPEN_ROW, ORPHAN_OPEN_ROW]);
    let predecessor: serde_json::Value = serde_json::from_str(SUPERSEDED_OPEN_ROW).unwrap();
    let open = orphan_open_line(&predecessor, &BTreeSet::new()).unwrap();
    append_seal_receipt(&bus, ChannelSealReason::Hangup, &open, &[]).unwrap();
    let before = std::fs::read(&bus).unwrap();
    let live = BTreeSet::from([ORPHAN_SESSION.to_string()]);
    assert!(
        seal_orphaned_sessions(std::slice::from_ref(&bus), &live, ORPHAN_SCAN_WINDOW_BYTES)
            .is_empty()
    );
    assert_eq!(
        std::fs::read(&bus).unwrap(),
        before,
        "a live successor is untouched"
    );
    let recovered = seal_orphaned_sessions(
        std::slice::from_ref(&bus),
        &BTreeSet::new(),
        ORPHAN_SCAN_WINDOW_BYTES,
    );
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].session_id, ORPHAN_SESSION,
        "the predecessor's close does not close its successor"
    );
}

/// The observed bug: channel 2's newest row stayed `open` after the app
/// quit, and the overlay painted a live microphone forever. One start
/// seals it with the key the overlay projection pairs on.
#[tokio::test]
async fn a_session_a_previous_process_left_open_gets_one_orphan_seal() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path(), CHANNEL_2_BINDING);
    let shared_bus = dir.path().join("transcript-events.jsonl");
    append_raw(&shared_bus, &[ORPHAN_OPEN_ROW, ORPHAN_TERMINAL_SEAL_ROW]);

    let sealed = controller
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;

    assert_eq!(
        sealed,
        vec![OrphanSeal {
            bus: shared_bus.clone(),
            channel: "2".to_string(),
            session_id: ORPHAN_SESSION.to_string(),
        }]
    );
    let rows = bus_rows(&shared_bus);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        sealed_rows(&rows),
        vec![(ORPHAN_SESSION.to_string(), "orphan".to_string())],
        "the named capture receives its own close"
    );
    let (open, seal) = (&rows[0], &rows[2]);
    assert_eq!(seal["schema"], "codescribe.channel-session.v1");
    assert_eq!(seal["kind"], "channel_session");
    assert_eq!(seal["state"], "sealed");
    assert_eq!(seal["reason"], "orphan");
    assert_eq!(seal["loud"], false);
    // OverlayChannelDelivery.Bus keys a channel's session on these and
    // lets a non-open row close it only when `opened_at` is the open
    // session's own, byte for byte.
    for key in [
        "channel",
        "agent",
        "session_id",
        "opened_at",
        "provider",
        "provider_session_id",
        "autoseal_secs",
        "utterance_silence_sec",
    ] {
        assert_eq!(seal[key], open[key], "{key}");
    }
    assert!(
        unclosed_channel_sessions(&shared_bus, ORPHAN_SCAN_WINDOW_BYTES)
            .expect("scan")
            .is_empty(),
        "the named capture is no longer open"
    );
}

#[tokio::test]
async fn channel_seal_records_whisper_timeout_and_disabled_status() {
    for (codes, status) in [
        (
            vec!["live_refinement_stop_deadline".to_string()],
            "refinement_incomplete",
        ),
        (
            vec!["agent_raw_whisper_disabled".to_string()],
            "whisper_disabled",
        ),
        (Vec::new(), "settled"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("channel.jsonl");
        let warnings = Arc::new(StdMutex::new(Vec::new()));
        let mut emitter =
            PresentationEmitter::new(Arc::new(TokioMutex::new(String::new())), None, None)
                .with_refinement_warnings(warnings.clone());
        for code in &codes {
            emitter.on_event(
                &codescribe_core::pipeline::contracts::EngineEvent::Warning {
                    code: code.clone(),
                    message: "refinement_not_completed=true".into(),
                },
            );
        }
        emitter.finish().await;
        let open = live_open_line("agent-raw-receipt");
        append_seal_receipt(
            &path,
            ChannelSealReason::Hangup,
            &open,
            &warnings.lock().unwrap(),
        )
        .unwrap();
        let rows = bus_rows(&path);
        assert_eq!(rows[0]["raw_processing_status"], status);
        assert_eq!(rows[0]["refinement_warnings"], serde_json::json!(codes));
    }
}

#[tokio::test]
async fn sessions_sealed_by_silence_or_hang_up_get_no_orphan_row() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    for (session, reason) in [
        ("agent-channel-3-silence", ChannelSealReason::Silence),
        ("agent-channel-3-hangup", ChannelSealReason::Hangup),
    ] {
        let open = live_open_line(session);
        write_open_row(&channel_bus, &open);
        append_seal_receipt(&channel_bus, reason, &open, &[]).expect("seal");
    }
    let before = std::fs::read(&channel_bus).expect("bus");

    let sealed = controller
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;

    assert!(sealed.is_empty(), "{sealed:?}");
    assert_eq!(std::fs::read(&channel_bus).expect("bus"), before);
    assert!(!shared_bus.exists());
}

#[tokio::test]
async fn a_second_start_writes_no_second_orphan_row() {
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path(), CHANNEL_2_BINDING);
    let shared_bus = dir.path().join("transcript-events.jsonl");
    append_raw(&shared_bus, &[ORPHAN_OPEN_ROW, ORPHAN_TERMINAL_SEAL_ROW]);

    let first = RecordingController::new_without_keychain()
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;
    let second = RecordingController::new_without_keychain()
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;

    assert_eq!(first.len(), 1, "{first:?}");
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(
        sealed_rows(&bus_rows(&shared_bus)),
        vec![(ORPHAN_SESSION.to_string(), "orphan".to_string())]
    );
}

/// W5 buses: the `open` row sits on the binding's dedicated bus, so the
/// orphan seal lands there and the shared bus is not touched.
#[tokio::test]
async fn an_open_row_on_a_dedicated_bus_is_sealed_on_that_bus() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    append_raw(&shared_bus, &[ORPHAN_TERMINAL_SEAL_ROW]);
    let shared_before = std::fs::read(&shared_bus).expect("shared bus");
    let open = live_open_line("agent-channel-3-orphan");
    write_open_row(&channel_bus, &open);

    let sealed = controller
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;

    assert_eq!(
        sealed,
        vec![OrphanSeal {
            bus: channel_bus.clone(),
            channel: "3".to_string(),
            session_id: "agent-channel-3-orphan".to_string(),
        }]
    );
    assert_eq!(
        sealed_rows(&bus_rows(&channel_bus)),
        vec![("agent-channel-3-orphan".to_string(), "orphan".to_string())]
    );
    assert_eq!(
        std::fs::read(&shared_bus).expect("shared bus"),
        shared_before
    );
}

/// A session this controller opened is live, not an orphan, even when its
/// `open` row is its channel's newest.
#[tokio::test]
#[serial_test::serial]
async fn a_session_this_process_owns_is_never_sealed_as_an_orphan() {
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let (binding, channel_bus) = write_dedicated_binding(dir.path());
    let shared_bus = dir.path().join("shared-bus.jsonl");
    open_stamped(&controller, &binding, &shared_bus, "agent-channel-3-live").await;
    write_open_row(&channel_bus, &live_open_line("agent-channel-3-live"));
    let before = std::fs::read(&channel_bus).expect("bus");

    let sealed = controller
        .seal_orphaned_channel_sessions(&shared_bus, &binding)
        .await;

    assert!(sealed.is_empty(), "{sealed:?}");
    assert_eq!(std::fs::read(&channel_bus).expect("bus"), before);
    assert!(controller.agent_channel_snapshot(3).await.is_some());
}

/// The scan reads a bounded tail and starts on a row boundary: a row the
/// window cuts is dropped whole, a row the window starts on is kept.
#[test]
fn the_orphan_scan_reads_only_whole_rows_of_the_bus_tail() {
    let dir = tempfile::tempdir().expect("temp");
    let bus = dir.path().join("bus.jsonl");
    let older = SUPERSEDED_OPEN_ROW.replace(r#""channel":"2""#, r#""channel":"4""#);
    append_raw(&bus, &[older.as_str(), ORPHAN_OPEN_ROW]);
    let newest = ORPHAN_OPEN_ROW.len() as u64 + 1;

    let exact = unclosed_channel_sessions(&bus, newest).expect("scan");
    assert_eq!(
        exact.keys().collect::<Vec<_>>(),
        vec![&(String::from("2"), String::from(ORPHAN_SESSION))]
    );
    let one_more = unclosed_channel_sessions(&bus, newest + 1).expect("scan");
    assert_eq!(one_more, exact);
    let cut = unclosed_channel_sessions(&bus, newest - 1).expect("scan");
    assert!(cut.is_empty(), "{cut:?}");
    let whole = unclosed_channel_sessions(&bus, ORPHAN_SCAN_WINDOW_BYTES).expect("scan");
    assert_eq!(
        whole
            .keys()
            .map(|(channel, _)| channel.as_str())
            .collect::<Vec<_>>(),
        vec!["2", "4"]
    );

    let sealed = seal_orphaned_sessions(std::slice::from_ref(&bus), &BTreeSet::new(), newest);
    assert_eq!(
        sealed
            .iter()
            .map(|seal| seal.channel.as_str())
            .collect::<Vec<_>>(),
        vec!["2"],
        "an open row outside the window is not read"
    );
    assert!(
        unclosed_channel_sessions(&dir.path().join("missing.jsonl"), newest)
            .expect("missing bus")
            .is_empty()
    );
}
include!("agent_channel_co_audience_tests.rs");
