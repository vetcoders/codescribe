//! Voice conversation with a Codescribe Bus agent (`settings.json`
//! `agent_channel` section) and the built-in Agent's own take timers.
//!
//! One pause rules a channel: the HUD countdown and the seal read the same
//! [`AgentChannelConfig::send_pause`] from the session's immutable snapshot.
//! `toggle_silence_sec` belongs to hands-free dictation only.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Power-user override of `agent_channel.send_pause_sec`. `0` turns automatic
/// sending (and the voiceless close) off.
pub const CHANNEL_AUTOSEAL_SECS_ENV: &str = "CODESCRIBE_CHANNEL_AUTOSEAL_SECS";

pub const DEFAULT_SEND_PAUSE_SEC: f32 = 5.0;
pub const SEND_PAUSE_RANGE: std::ops::RangeInclusive<f32> = 1.0..=15.0;
/// Voiceless close for `end_after_message`; `continuous` defaults to never.
pub const DEFAULT_END_AFTER_MESSAGE_VOICELESS_SEC: u64 = 30;
pub const VOICELESS_CLOSE_RANGE: std::ops::RangeInclusive<u64> = 15..=600;
pub const DEFAULT_FORMAT_WAIT_SEC: u64 = 3;
pub const FORMAT_WAIT_RANGE: std::ops::RangeInclusive<u64> = 1..=10;
pub const DEFAULT_AGENT_AUTO_SEND_DELAY_SEC: u64 = 5;
pub const AGENT_AUTO_SEND_DELAY_RANGE: std::ops::RangeInclusive<u64> = 1..=30;
pub const DEFAULT_AGENT_TAKE_VOICELESS_END_SEC: u64 = 30;
pub const AGENT_TAKE_VOICELESS_END_RANGE: std::ops::RangeInclusive<u64> = 10..=120;

/// What a pause after heard voice does to an open channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSessionPolicy {
    /// Deliver the message and keep listening on the same session.
    #[default]
    Continuous,
    /// Deliver the message and end the session, as a hang-up does.
    EndAfterMessage,
}

impl ChannelSessionPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::EndAfterMessage => "end_after_message",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "continuous" => Some(Self::Continuous),
            "end_after_message" => Some(Self::EndAfterMessage),
            _ => None,
        }
    }

    /// Voiceless close a policy starts with: never for a conversation that
    /// lasts until hang-up, thirty seconds for one message per press.
    pub const fn default_voiceless_close_sec(self) -> u64 {
        match self {
            Self::Continuous => 0,
            Self::EndAfterMessage => DEFAULT_END_AFTER_MESSAGE_VOICELESS_SEC,
        }
    }
}

/// How a sealed channel message is finished before delivery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSegmentFinish {
    /// The committed segment text, byte for byte.
    Off,
    /// Terminal punctuation only; earlier bytes unchanged.
    #[default]
    LightPlus,
    /// One formatter pass at `speech.formatting.level`, bounded by
    /// `format_wait_sec`; Light+ text is delivered when it does not land.
    Formatter,
}

impl ChannelSegmentFinish {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::LightPlus => "light_plus",
            Self::Formatter => "formatter",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "off" => Some(Self::Off),
            "light_plus" => Some(Self::LightPlus),
            "formatter" => Some(Self::Formatter),
            _ => None,
        }
    }
}

/// Where the effective send pause came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSettingSource {
    #[default]
    Settings,
    EnvOverride,
}

impl ChannelSettingSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::EnvOverride => "env_override",
        }
    }
}

/// Resolved channel settings for one runtime generation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentChannelConfig {
    pub session_policy: ChannelSessionPolicy,
    /// Seconds of silence after heard voice that send the message. `0` only
    /// through [`CHANNEL_AUTOSEAL_SECS_ENV`]: automatic sending is off.
    pub send_pause_sec: f32,
    pub send_pause_source: ChannelSettingSource,
    /// `0` = never; otherwise within [`VOICELESS_CLOSE_RANGE`].
    pub voiceless_close_sec: u64,
    pub segment_finish: ChannelSegmentFinish,
    pub format_wait_sec: u64,
}

impl Default for AgentChannelConfig {
    fn default() -> Self {
        let session_policy = ChannelSessionPolicy::default();
        Self {
            session_policy,
            send_pause_sec: DEFAULT_SEND_PAUSE_SEC,
            send_pause_source: ChannelSettingSource::Settings,
            voiceless_close_sec: session_policy.default_voiceless_close_sec(),
            segment_finish: ChannelSegmentFinish::default(),
            format_wait_sec: DEFAULT_FORMAT_WAIT_SEC,
        }
    }
}

impl AgentChannelConfig {
    /// The pause that seals a message; `None` = automatic sending is off.
    pub fn send_pause(&self) -> Option<Duration> {
        (self.send_pause_sec.is_finite() && self.send_pause_sec > 0.0)
            .then(|| Duration::from_secs_f32(self.send_pause_sec))
    }

    /// Voiceless close, or `None` when it never fires. Automatic sending off
    /// turns it off too, as the env override always has.
    pub fn voiceless_close(&self) -> Option<Duration> {
        (self.voiceless_close_sec > 0 && self.send_pause().is_some())
            .then(|| Duration::from_secs(self.voiceless_close_sec))
    }

    pub fn format_wait(&self) -> Duration {
        Duration::from_secs(self.format_wait_sec)
    }

    /// Clamp persisted values into their ranges. The env override is the
    /// power user's explicit number and is kept as given, `0` included.
    pub fn sanitize(&mut self) {
        if !self.send_pause_sec.is_finite() || self.send_pause_sec < 0.0 {
            self.send_pause_sec = DEFAULT_SEND_PAUSE_SEC;
        }
        if self.send_pause_source == ChannelSettingSource::Settings {
            self.send_pause_sec = normalize_send_pause_sec(self.send_pause_sec);
        }
        self.voiceless_close_sec = normalize_voiceless_close_sec(self.voiceless_close_sec);
        self.format_wait_sec = self
            .format_wait_sec
            .clamp(*FORMAT_WAIT_RANGE.start(), *FORMAT_WAIT_RANGE.end());
    }

    /// One-line description for "active configuration" diagnostics.
    pub fn describe(&self) -> String {
        format!(
            "session_policy={} send_pause_sec={} ({}) voiceless_close_sec={} segment_finish={} format_wait_sec={}",
            self.session_policy.as_str(),
            self.send_pause_sec,
            self.send_pause_source.as_str(),
            self.voiceless_close_sec,
            self.segment_finish.as_str(),
            self.format_wait_sec,
        )
    }
}

/// Into `1.0..=15.0`, on the slider's half-second grid.
pub fn normalize_send_pause_sec(seconds: f32) -> f32 {
    if !seconds.is_finite() {
        return DEFAULT_SEND_PAUSE_SEC;
    }
    ((seconds * 2.0).round() / 2.0).clamp(*SEND_PAUSE_RANGE.start(), *SEND_PAUSE_RANGE.end())
}

/// `0` stays never; anything else lands in `15..=600`.
pub fn normalize_voiceless_close_sec(seconds: u64) -> u64 {
    if seconds == 0 {
        0
    } else {
        seconds.clamp(*VOICELESS_CLOSE_RANGE.start(), *VOICELESS_CLOSE_RANGE.end())
    }
}

/// Parse the env override: whole or fractional seconds, `0` = off. Unreadable
/// values are no override.
pub fn parse_autoseal_override(raw: &str) -> Option<f32> {
    let value: f32 = raw.trim().parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_a_continuous_conversation_with_a_five_second_pause() {
        let config = AgentChannelConfig::default();
        assert_eq!(config.session_policy, ChannelSessionPolicy::Continuous);
        assert_eq!(config.send_pause(), Some(Duration::from_secs(5)));
        assert_eq!(config.voiceless_close(), None);
        assert_eq!(config.segment_finish, ChannelSegmentFinish::LightPlus);
        assert_eq!(config.format_wait(), Duration::from_secs(3));
        assert_eq!(
            ChannelSessionPolicy::EndAfterMessage.default_voiceless_close_sec(),
            30
        );
    }

    #[test]
    fn sanitize_clamps_ranges_and_keeps_an_env_zero() {
        let mut config = AgentChannelConfig {
            send_pause_sec: 40.2,
            voiceless_close_sec: 3,
            format_wait_sec: 0,
            ..AgentChannelConfig::default()
        };
        config.sanitize();
        assert_eq!(config.send_pause_sec, 15.0);
        let mut env = AgentChannelConfig {
            send_pause_sec: 40.0,
            send_pause_source: ChannelSettingSource::EnvOverride,
            ..AgentChannelConfig::default()
        };
        env.sanitize();
        assert_eq!(env.send_pause_sec, 40.0, "the env number is kept as given");
        assert_eq!(config.voiceless_close_sec, 15);
        assert_eq!(config.format_wait_sec, 1);

        let mut off = AgentChannelConfig {
            send_pause_sec: 0.0,
            send_pause_source: ChannelSettingSource::EnvOverride,
            voiceless_close_sec: 60,
            ..AgentChannelConfig::default()
        };
        off.sanitize();
        assert_eq!(off.send_pause(), None);
        assert_eq!(off.voiceless_close(), None, "env 0 turns both off");
        assert_eq!(normalize_send_pause_sec(4.26), 4.5);
        assert_eq!(normalize_voiceless_close_sec(0), 0);
        assert_eq!(normalize_voiceless_close_sec(9_999), 600);
    }

    #[test]
    fn wire_names_round_trip() {
        for policy in [
            ChannelSessionPolicy::Continuous,
            ChannelSessionPolicy::EndAfterMessage,
        ] {
            assert_eq!(ChannelSessionPolicy::parse(policy.as_str()), Some(policy));
        }
        for finish in [
            ChannelSegmentFinish::Off,
            ChannelSegmentFinish::LightPlus,
            ChannelSegmentFinish::Formatter,
        ] {
            assert_eq!(ChannelSegmentFinish::parse(finish.as_str()), Some(finish));
        }
        assert_eq!(ChannelSessionPolicy::parse("sometimes"), None);
        assert_eq!(parse_autoseal_override(" 4.5 "), Some(4.5));
        assert_eq!(parse_autoseal_override("nope"), None);
        assert_eq!(parse_autoseal_override("-1"), None);
    }
}
