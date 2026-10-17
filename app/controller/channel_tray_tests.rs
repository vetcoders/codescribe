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

/// The tray is process-global: every test leaves it the way it found it.
struct TrayRestore(TrayStatusSnapshot, bool);

impl TrayRestore {
    fn capture() -> Self {
        // Non-serial neighbours may have opened channels without closing
        // them; this file owns the flag's truth for its own assertions.
        tray_status::set_agent_channels_listening(false);
        Self(
            tray_status::current_tray_status_snapshot(),
            is_assistive_session(),
        )
    }
}

impl Drop for TrayRestore {
    fn drop(&mut self) {
        tray_status::set_agent_channels_listening(false);
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
#[serial_test::serial]
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
#[serial_test::serial]
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

// Third falsifier (pending the integrator's cut shape): a late `Idle` from
// the controller state forwarder, arriving after a channel reopened (silence
// seal → reopen, or a dictation handoff), must not blacken the dot of a live
// channel. It lands next to the guard that implements it (bridge forwarder or
// controller), since the seam decides where it can be driven from a test.
