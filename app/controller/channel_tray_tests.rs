//! Falsifiers for the menu bar dot during agent channel sessions.
//!
//! Maciej, 2026-10-17 00:45Z (channel 0 seal, retranscribed): "kropeczka w
//! glyphie w treju pozostaje czarna, jak mówię do agentów, powinna być
//! fioletowa; kursor OK". A channel session never moves the controller out of
//! `Idle`, so `forward_controller_state_to_listener` never published
//! `Listening`, `set_tray_voice_for_take` bailed on `status != Listening`, and
//! the tray kept its monochrome template dot while the pointer dot already wore
//! Agent violet. These pin the tray to the channel lifecycle the way the
//! pointer badge already is.
use super::agent_channel::ChannelOpenMode;
use super::*;
use crate::os::hold_badge::BadgeMode;
use crate::os::tray_status::{self, TrayStatus, TrayStatusSnapshot};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The tray is process-global: every test leaves it the way it found it.
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

fn write_binding(dir: &Path) -> PathBuf {
    let path = dir.join("agent-channel-binding.json");
    std::fs::write(
        &path,
        r#"{"schema":"vc.agent-audience-binding.v1","bindings":{"3":{"audience":"tray-agent","provider":"codex","provider_session_id":"tray-session"}}}"#,
    )
    .expect("binding");
    path
}

/// A settled dictation tray: idle, hold lane, as after any ordinary take.
fn settle_idle_hold() {
    set_assistive_session(false);
    tray_status::set_tray_indicator_mode(BadgeMode::Hold);
    tray_status::update_tray_status(TrayStatus::Idle);
}

/// Opening a channel at `Idle` must paint the tray dot Agent violet, exactly
/// like the pointer dot: `Listening` on the `Assistive` lane.
#[tokio::test]
#[serial_test::serial(agent_ack_duck)]
async fn an_open_channel_paints_the_tray_dot_agent_violet() {
    let _restore = TrayRestore::capture();
    settle_idle_hold();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path());
    let bus = dir.path().join("bus.jsonl");

    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");

    assert_eq!(
        controller.current_state().await,
        State::Idle,
        "a channel session never leaves the controller Idle; the tray cannot wait for a StateChange"
    );
    let snapshot = tray_status::current_tray_status_snapshot();
    assert_eq!(
        snapshot.status,
        TrayStatus::Listening,
        "the tray must hear the channel the way the pointer badge does"
    );
    assert_eq!(
        snapshot.indicator_mode,
        BadgeMode::Assistive,
        "Agent violet on the glyph, not the recording red and not the template dot"
    );

    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::Live)
        .await
        .expect("second press hangs up");
    assert!(controller.agent_channel_snapshot(3).await.is_none());
}

/// Hanging up the last channel at `Idle` takes the dot with it: the tray goes
/// back to the template dot instead of a violet that outlives the session.
#[tokio::test]
#[serial_test::serial(agent_ack_duck)]
async fn hanging_up_the_last_channel_settles_the_tray_dot() {
    let _restore = TrayRestore::capture();
    settle_idle_hold();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path());
    let bus = dir.path().join("bus.jsonl");
    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    assert_eq!(tray_status::current_tray_status(), TrayStatus::Listening);

    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::Live)
        .await
        .expect("hang up");

    assert!(controller.agent_channel_snapshot(3).await.is_none());
    assert_eq!(controller.current_state().await, State::Idle);
    assert_eq!(
        tray_status::current_tray_status(),
        TrayStatus::Idle,
        "no channel and no take means no colored dot"
    );
}

/// A finished or cancelled take publishes `Idle` through the bridge's state
/// forwarder (`forward_controller_state_to_listener`). While a channel still
/// listens, that foreign `Idle` must resolve to the channel's own Agent violet
/// instead of blackening the dot in the middle of a conversation.
#[tokio::test]
#[serial_test::serial(agent_ack_duck)]
async fn a_foreign_idle_cannot_blacken_a_live_channel() {
    let _restore = TrayRestore::capture();
    settle_idle_hold();
    let controller = RecordingController::new_without_keychain();
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path());
    let bus = dir.path().join("bus.jsonl");
    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");

    // Exactly what the bridge forwarder publishes for `State::Idle`.
    tray_status::update_tray_status(TrayStatus::Idle);

    let snapshot = tray_status::current_tray_status_snapshot();
    assert_eq!(
        snapshot.status,
        TrayStatus::Listening,
        "a take's Idle must not outrank a channel that still listens"
    );
    assert_eq!(snapshot.indicator_mode, BadgeMode::Assistive);

    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::Live)
        .await
        .expect("hang up");
    assert_eq!(
        tray_status::current_tray_status(),
        TrayStatus::Idle,
        "the hang-up is the one Idle that goes through"
    );
}

/// The mirror-image bug: an ordinary take admitted over a channel closes that
/// channel (dictation handoff), so once the take ends its `Idle` must go
/// through. A violet that outlives the channel would be as wrong as the black
/// dot this file started from.
#[tokio::test]
#[serial_test::serial(agent_ack_duck)]
async fn a_dictation_handoff_lets_the_takes_idle_through() {
    let _restore = TrayRestore::capture();
    settle_idle_hold();
    let controller = Arc::new(RecordingController::new_without_keychain());
    let dir = tempfile::tempdir().expect("temp");
    let binding = write_binding(dir.path());
    let bus = dir.path().join("bus.jsonl");
    controller
        .dispatch_agent_channel(3, &binding, &bus, ChannelOpenMode::AttachedOnly)
        .await
        .expect("open");
    assert_eq!(tray_status::current_tray_status(), TrayStatus::Listening);

    controller
        .handle_hotkey_event(HotkeyInput {
            key_type: HotkeyType::Hold,
            action: HotkeyAction::Down,
            assistive: false,
            hold_mode: HoldMode::Raw,
            force_raw: false,
            force_ai: false,
        })
        .await
        .expect("ordinary start");
    tokio::time::timeout(Duration::from_secs(5), async {
        while controller.current_state().await != State::RecHold {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded fake recorder admission");
    assert!(
        controller.agent_channel_snapshot(3).await.is_none(),
        "the handoff hung up the channel before the take was admitted"
    );

    controller.reset().await;
    assert_eq!(controller.current_state().await, State::Idle);
    // The take's end, as the bridge forwarder publishes it.
    tray_status::update_tray_status(TrayStatus::Idle);
    assert_eq!(
        tray_status::current_tray_status(),
        TrayStatus::Idle,
        "no channel is left to hold the violet"
    );
}
