//! Channel-capture gate while agent speech owns the speaker.
//!
//! The gate is a deadline. It drops channel PCM for that window and then
//! expires. Take feeds are never consulted here.
//!
//! Agent replies on the bus are played by `bus-demux` through `afplay`, a
//! process outside this one. While that player lives, the microphone on this
//! Mac hears an agent, not the Founder (Maciej, 2026-10-22: "ktoś z nas użył
//! afplay ... ja nie używam afplaya, jak mówię"). The speaking process
//! announces the reply in [`EXTERNAL_PLAYBACK_MARKER`], and
//! [`watch_external_playback`] keeps the gate armed while that process lives
//! and the reply has not ended, plus a room tail.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static START: OnceLock<Instant> = OnceLock::new();
static UNTIL_MS: AtomicU64 = AtomicU64::new(0);

/// Tail applied when a Bus `agent_reply` arrives with `spoken: true`.
///
/// That row is written after the external speaker has already returned, so
/// this window covers the tail of the utterance rather than the utterance
/// itself. In-process playback arms the gate for the real play duration.
pub const SPOKEN_REPLY_POST_ROLL: Duration = Duration::from_millis(750);

fn mono_ms() -> u64 {
    let millis = START.get_or_init(Instant::now).elapsed().as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

/// Extend the channel-capture gate through `duration`.
pub fn arm_for(duration: Duration) {
    let extra = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let until = mono_ms().saturating_add(extra);
    UNTIL_MS.fetch_max(until, Ordering::SeqCst);
}

/// Drop the gate immediately.
pub fn clear() {
    UNTIL_MS.store(0, Ordering::SeqCst);
}

/// Whether a channel feed should discard the block it was just offered.
pub fn channel_capture_should_drop() -> bool {
    let until = UNTIL_MS.load(Ordering::SeqCst);
    until > 0 && mono_ms() < until
}

/// Arm the post-roll when a spoken agent reply is observed.
pub fn observe_spoken_reply(spoken: bool) {
    if spoken {
        arm_for(SPOKEN_REPLY_POST_ROLL);
    }
}

/// File `bus-demux` writes under the bridge `runtime/` directory before the
/// first sample. When the reply ends it rewrites `until_ms` to that moment.
pub const EXTERNAL_PLAYBACK_MARKER: &str = "agent-playback-now.json";
/// Schema of [`EXTERNAL_PLAYBACK_MARKER`].
pub const EXTERNAL_PLAYBACK_SCHEMA: &str = "codescribe.agent-playback-now.v1";
/// Room reverberation the microphone still hears after the player stops.
pub const EXTERNAL_PLAYBACK_TAIL: Duration = Duration::from_millis(300);
/// How often the marker is read; each live read re-arms the tail.
const EXTERNAL_PLAYBACK_POLL: Duration = Duration::from_millis(40);

static EXTERNAL_WATCH_STARTED: AtomicBool = AtomicBool::new(false);

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(target_os = "macos"))]
fn pid_alive(pid: u32) -> bool {
    pid > 0
}

/// Whether the marker at `marker` names a reply still playing at `now_ms`.
///
/// The speaking process must be alive and the announced end not yet passed,
/// so a marker left behind by a crash or a reused pid cannot mute the channel
/// for longer than the reply it described.
pub fn external_playback_active(marker: &Path, now_ms: u64) -> bool {
    let Ok(raw) = std::fs::read(marker) else {
        return false;
    };
    if raw.len() > 4096 {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return false;
    };
    if value.get("schema").and_then(serde_json::Value::as_str) != Some(EXTERNAL_PLAYBACK_SCHEMA) {
        return false;
    }
    let pid = value.get("pid").and_then(serde_json::Value::as_u64);
    let until_ms = value.get("until_ms").and_then(serde_json::Value::as_u64);
    match (pid.and_then(|pid| u32::try_from(pid).ok()), until_ms) {
        (Some(pid), Some(until_ms)) => now_ms < until_ms && pid_alive(pid),
        _ => false,
    }
}

/// Keep the gate armed while a reply announced in `marker` plays.
///
/// Started once per process; later calls return without a second watcher.
pub fn watch_external_playback(marker: PathBuf) {
    if EXTERNAL_WATCH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("tts-duck-external".to_string())
        .spawn(move || {
            loop {
                if external_playback_active(&marker, unix_ms(SystemTime::now())) {
                    arm_for(EXTERNAL_PLAYBACK_TAIL + EXTERNAL_PLAYBACK_POLL);
                }
                std::thread::sleep(EXTERNAL_PLAYBACK_POLL);
            }
        });
    if let Err(error) = spawned {
        EXTERNAL_WATCH_STARTED.store(false, Ordering::SeqCst);
        tracing::warn!(%error, "external playback watcher could not start");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial(tts_duck)]
    fn the_gate_is_a_deadline() {
        clear();
        assert!(!channel_capture_should_drop());
        arm_for(Duration::from_secs(30));
        assert!(channel_capture_should_drop());
        clear();
        assert!(!channel_capture_should_drop());
        arm_for(Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            !channel_capture_should_drop(),
            "a spoken reply must not mute the channel after the window"
        );
        observe_spoken_reply(false);
        assert!(!channel_capture_should_drop());
        observe_spoken_reply(true);
        assert!(channel_capture_should_drop());
        clear();
    }

    fn write_marker(dir: &Path, pid: u32, until_ms: u64) -> PathBuf {
        let marker = dir.join(EXTERNAL_PLAYBACK_MARKER);
        std::fs::write(
            &marker,
            serde_json::json!({
                "schema": EXTERNAL_PLAYBACK_SCHEMA, "pid": pid, "until_ms": until_ms,
            })
            .to_string(),
        )
        .unwrap();
        marker
    }

    /// A marker counts only while its player lives and its end lies ahead.
    #[test]
    fn external_playback_needs_a_live_player_before_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let now = unix_ms(SystemTime::now());
        let me = std::process::id();
        assert!(!external_playback_active(
            &dir.path().join(EXTERNAL_PLAYBACK_MARKER),
            now
        ));
        let marker = write_marker(dir.path(), me, now + 5_000);
        assert!(external_playback_active(&marker, now));
        assert!(
            !external_playback_active(&marker, now + 5_000),
            "a marker past its announced end is a leftover"
        );
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let marker = write_marker(dir.path(), dead, now + 5_000);
        assert!(
            !external_playback_active(&marker, now),
            "a dead player's marker must not mute the channel"
        );
        std::fs::write(
            &marker,
            br#"{"schema":"other","pid":1,"until_ms":99999999999999}"#,
        )
        .unwrap();
        assert!(!external_playback_active(&marker, now));
    }

    /// The watcher arms the gate while the player lives and lets it lapse
    /// within the room tail once the marker is gone.
    #[test]
    #[serial(tts_duck)]
    fn external_watcher_arms_the_gate_for_the_player_only() {
        clear();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(EXTERNAL_PLAYBACK_MARKER);
        watch_external_playback(marker.clone());
        assert!(!channel_capture_should_drop());
        write_marker(
            dir.path(),
            std::process::id(),
            unix_ms(SystemTime::now()) + 10_000,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !channel_capture_should_drop() && Instant::now() < deadline {
            std::thread::sleep(EXTERNAL_PLAYBACK_POLL);
        }
        assert!(
            channel_capture_should_drop(),
            "a live player drops channel PCM"
        );
        std::fs::remove_file(&marker).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while channel_capture_should_drop() && Instant::now() < deadline {
            std::thread::sleep(EXTERNAL_PLAYBACK_POLL);
        }
        assert!(
            !channel_capture_should_drop(),
            "the gate lapses after the room tail once the player is gone"
        );
        clear();
    }
}
