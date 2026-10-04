//! Channel-capture gate while agent speech owns the speaker.
//!
//! The gate is a deadline. It drops channel PCM for that window and then
//! expires. Take feeds are never consulted here.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
}
