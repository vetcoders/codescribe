//! Measured speech phases for one capture, from PCM energy and Silero VAD
//! edges on its capture epoch. Recognizer output timing is never an input.
//!
//! Agent channels and the hands-free assistive take read the same clock; each
//! asks its own question of it (`silence_due` and `voiceless_due` for a
//! channel, `no_voice_for` for the assistive take).

use std::time::{Duration, SystemTime};

use codescribe_core::pipeline::contracts::{SidebandEvidence, SidebandEvidenceKind};

/// A capture that hears no voice for this many seconds ends, so a start
/// without a message never leaves the microphone live (Founder decisions
/// 2026-10-26: agent channels, then the assistive take). Agent playback, which
/// a capture cannot tell from voice, does not count as silence.
pub const VOICELESS_CLOSE_SECS: u64 = 30;

pub(crate) fn silence_is_due(last_voice: SystemTime, now: SystemTime, secs: u64) -> bool {
    if secs == 0 {
        return false;
    }
    now.duration_since(last_voice).unwrap_or_default() >= Duration::from_secs(secs)
}

#[derive(Debug, Default)]
enum SpeechPhase {
    #[default]
    Unmeasured,
    Speaking,
    Quiet {
        since: SystemTime,
    },
}

#[derive(Debug)]
pub(crate) struct SpeechEdgeCursor {
    capture_epoch: u64,
    pub(crate) sequence: u64,
    pub(crate) sample: u64,
    sample_rate_hz: u32,
}

/// Cadence follows measured speech edges, never recognizer output timing.
#[derive(Debug, Default)]
pub(crate) struct SpeechClock {
    pub(crate) last_edge: Option<SpeechEdgeCursor>,
    phase: SpeechPhase,
    /// When the last accepted edge was received.
    last_edge_at: Option<SystemTime>,
    /// Last poll that saw agent playback; the voiceless limit restarts there.
    voiceless_from: Option<SystemTime>,
}

impl SpeechClock {
    /// The caller has already checked the opened session and capture owner.
    pub(crate) fn observe(&mut self, evidence: &SidebandEvidence, received_at: SystemTime) {
        let speaking = match &evidence.evidence {
            SidebandEvidenceKind::SpeechStart { .. } => true,
            SidebandEvidenceKind::SpeechEnd { .. } => false,
            SidebandEvidenceKind::Pause { .. } => return,
        };
        let range = &evidence.range;
        // Speech edges are zero-width boundaries, including at sample zero.
        if range.capture_epoch == 0
            || evidence.sample_rate_hz == 0
            || range.sample_start != range.sample_end
            || self.last_edge.as_ref().is_some_and(|previous| {
                previous.capture_epoch != range.capture_epoch
                    || previous.sample_rate_hz != evidence.sample_rate_hz
                    || evidence.sequence <= previous.sequence
                    || range.sample_start < previous.sample
            })
        {
            return;
        }
        let same_boundary = self
            .last_edge
            .as_ref()
            .is_some_and(|previous| previous.sample == range.sample_start);
        self.last_edge = Some(SpeechEdgeCursor {
            capture_epoch: range.capture_epoch,
            sequence: evidence.sequence,
            sample: range.sample_start,
            sample_rate_hz: evidence.sample_rate_hz,
        });
        self.last_edge_at = Some(received_at);
        if speaking {
            self.phase = SpeechPhase::Speaking;
        } else {
            match &self.phase {
                SpeechPhase::Speaking => {
                    // Receipt time cannot backdate measured silence.
                    self.phase = SpeechPhase::Quiet { since: received_at };
                }
                SpeechPhase::Quiet { .. } if !same_boundary => {
                    // A different end without its start is an incomplete edge
                    // chain, not proof of continuous silence since the old end.
                    self.phase = SpeechPhase::Unmeasured;
                }
                _ => {}
            }
        }
    }

    /// `fresh` has seen no edge since the capture opened; `unmeasured` is a
    /// broken edge chain and is never reported as quiet.
    pub(crate) fn phase_label(&self) -> &'static str {
        match (&self.phase, &self.last_edge) {
            (SpeechPhase::Unmeasured, None) => "fresh",
            (SpeechPhase::Unmeasured, Some(_)) => "unmeasured",
            (SpeechPhase::Speaking, _) => "speaking",
            (SpeechPhase::Quiet { .. }, _) => "quiet",
        }
    }

    pub(crate) fn quiet_since(&self) -> Option<SystemTime> {
        match &self.phase {
            SpeechPhase::Quiet { since } => Some(*since),
            SpeechPhase::Unmeasured | SpeechPhase::Speaking => None,
        }
    }

    pub(crate) fn silence_due(&self, now: SystemTime, secs: u64) -> bool {
        self.quiet_since()
            .is_some_and(|since| silence_is_due(since, now, secs))
    }

    /// Agent playback cannot be told from voice, and a channel feed hears
    /// nothing while it plays: that time is not voiceless.
    pub(crate) fn note_agent_playback(&mut self, now: SystemTime) {
        self.voiceless_from = Some(now);
    }

    fn voiceless_start(&self, opened_at: SystemTime) -> SystemTime {
        self.voiceless_from
            .map_or(opened_at, |from| from.max(opened_at))
    }

    /// No edge since the capture opened, and [`VOICELESS_CLOSE_SECS`] have
    /// passed since it opened or since the last agent playback, whichever is
    /// later. `secs == 0` (the channel threshold turned off) turns it off too.
    pub(crate) fn voiceless_due(&self, opened_at: SystemTime, now: SystemTime, secs: u64) -> bool {
        secs != 0
            && matches!(
                (&self.phase, &self.last_edge),
                (SpeechPhase::Unmeasured, None)
            )
            && silence_is_due(self.voiceless_start(opened_at), now, VOICELESS_CLOSE_SECS)
    }

    /// No voice for `secs`: not speaking, and `secs` have passed since the
    /// latest of the open, the last speech edge and the last agent playback.
    /// A broken edge chain reads as not speaking, so it cannot hold a capture
    /// open; only a measured start without its end does.
    pub(crate) fn no_voice_for(&self, opened_at: SystemTime, now: SystemTime, secs: u64) -> bool {
        if matches!(self.phase, SpeechPhase::Speaking) {
            return false;
        }
        let from = self
            .last_edge_at
            .map_or(self.voiceless_start(opened_at), |edge| {
                edge.max(self.voiceless_start(opened_at))
            });
        silence_is_due(from, now, secs)
    }

    pub(crate) fn deadline(&self, secs: u64) -> Option<SystemTime> {
        if secs == 0 {
            return None;
        }
        self.quiet_since()?.checked_add(Duration::from_secs(secs))
    }
}

#[cfg(test)]
pub(crate) fn measured_clock_edge(seq: u64, sample: u64, speech: bool) -> SidebandEvidence {
    SidebandEvidence {
        sequence: seq,
        range: codescribe_core::stt::tail_provider::TailSampleRange {
            session: "clock-owner".into(),
            capture_epoch: 1,
            sample_start: sample,
            sample_end: sample,
        },
        sample_rate_hz: 16_000,
        provenance: codescribe_core::pipeline::contracts::SidebandProvenance::SileroVad,
        evidence: if speech {
            SidebandEvidenceKind::SpeechStart {
                speech_probability: 0.95,
            }
        } else {
            SidebandEvidenceKind::SpeechEnd {
                speech_probability: 0.05,
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(opened: SystemTime, secs: u64) -> SystemTime {
        opened + Duration::from_secs(secs)
    }

    #[test]
    fn silence_is_due_counts_whole_seconds_and_zero_turns_it_off() {
        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(!silence_is_due(opened, at(opened, 119), 120));
        assert!(silence_is_due(opened, at(opened, 120), 120));
        assert!(!silence_is_due(opened, at(opened, 10_000), 0));
    }

    #[test]
    fn a_capture_that_never_hears_voice_has_none_for_thirty_seconds() {
        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let clock = SpeechClock::default();
        assert!(!clock.no_voice_for(opened, at(opened, 29), VOICELESS_CLOSE_SECS));
        assert!(clock.no_voice_for(opened, at(opened, 30), VOICELESS_CLOSE_SECS));
    }

    #[test]
    fn speech_holds_the_capture_and_its_end_starts_the_thirty_seconds() {
        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut clock = SpeechClock::default();
        clock.observe(&measured_clock_edge(1, 16_000, true), at(opened, 1));
        assert!(
            !clock.no_voice_for(opened, at(opened, 600), VOICELESS_CLOSE_SECS),
            "a measured start without its end is voice, however long"
        );
        clock.observe(&measured_clock_edge(2, 9_600_000, false), at(opened, 601));
        assert!(!clock.no_voice_for(opened, at(opened, 630), VOICELESS_CLOSE_SECS));
        assert!(clock.no_voice_for(opened, at(opened, 631), VOICELESS_CLOSE_SECS));
    }

    #[test]
    fn agent_playback_is_not_silence_and_restarts_the_thirty_seconds() {
        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut clock = SpeechClock::default();
        clock.note_agent_playback(at(opened, 25));
        assert!(!clock.no_voice_for(opened, at(opened, 54), VOICELESS_CLOSE_SECS));
        assert!(clock.no_voice_for(opened, at(opened, 55), VOICELESS_CLOSE_SECS));
        clock.observe(&measured_clock_edge(1, 16_000, true), at(opened, 56));
        clock.observe(&measured_clock_edge(2, 32_000, false), at(opened, 57));
        clock.note_agent_playback(at(opened, 70));
        assert!(
            !clock.no_voice_for(opened, at(opened, 99), VOICELESS_CLOSE_SECS),
            "playback after the last end restarts the count again"
        );
        assert!(clock.no_voice_for(opened, at(opened, 100), VOICELESS_CLOSE_SECS));
    }

    #[test]
    fn a_broken_edge_chain_cannot_hold_the_capture_open() {
        let opened = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut clock = SpeechClock::default();
        clock.observe(&measured_clock_edge(1, 16_000, true), at(opened, 1));
        clock.observe(&measured_clock_edge(2, 32_000, false), at(opened, 2));
        clock.observe(&measured_clock_edge(3, 48_000, false), at(opened, 3));
        assert_eq!(clock.phase_label(), "unmeasured");
        assert!(!clock.no_voice_for(opened, at(opened, 32), VOICELESS_CLOSE_SECS));
        assert!(
            clock.no_voice_for(opened, at(opened, 33), VOICELESS_CLOSE_SECS),
            "thirty seconds after the last edge, whatever the phase label"
        );
        assert!(
            !clock.voiceless_due(opened, at(opened, 600), 5),
            "the channel's voiceless rule still needs a capture with no edge"
        );
    }

    #[test]
    fn continuous_speech_outlives_sparse_recognizer_output_and_only_a_real_end_arms_silence() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = SpeechClock::default();
        assert_eq!(clock.deadline(5), None);
        assert!(!clock.silence_due(at(start, 1000), 5));
        clock.observe(&measured_clock_edge(1, 0, true), start);
        // No recognizer output for twenty seconds is not twenty seconds of silence.
        assert!(!clock.silence_due(at(start, 20), 5));
        assert_eq!(clock.deadline(5), None);
        let ended = at(start, 21);
        clock.observe(&measured_clock_edge(2, 336_000, false), ended);
        assert_eq!(clock.deadline(5), Some(ended + Duration::from_secs(5)));
        assert!(!clock.silence_due(ended + Duration::from_secs(4), 5));
        assert!(clock.silence_due(ended + Duration::from_secs(5), 5));
        assert!(!clock.silence_due(ended + Duration::from_secs(99), 0));
        assert!(!clock.silence_due(start, 5));
    }

    #[test]
    fn regressions_and_incomplete_edges_never_invent_a_valley() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = SpeechClock::default();
        clock.observe(&measured_clock_edge(1, 0, false), t);
        assert_eq!(clock.deadline(5), None, "first end lacks its start");
        clock.observe(&measured_clock_edge(2, 0, true), t);
        clock.observe(&measured_clock_edge(3, 16_000, false), t);
        let due = clock.deadline(5);
        clock.observe(&measured_clock_edge(4, 16_000, false), at(t, 4));
        assert_eq!(
            clock.deadline(5),
            due,
            "repeated end cannot postpone delivery"
        );
        clock.observe(&measured_clock_edge(2, 0, true), t);
        clock.observe(&measured_clock_edge(5, 0, true), t);
        let mut bad_rate = measured_clock_edge(6, 32_000, true);
        bad_rate.sample_rate_hz = 48_000;
        clock.observe(&bad_rate, t);
        let mut bad_epoch = measured_clock_edge(6, 32_000, true);
        bad_epoch.range.capture_epoch = 2;
        clock.observe(&bad_epoch, t);
        assert_eq!(
            clock.deadline(5),
            due,
            "regressing or foreign units leave the accepted boundary intact"
        );
        clock.observe(&measured_clock_edge(7, 32_000, false), t);
        assert_eq!(
            clock.deadline(5),
            None,
            "another end without its start is unmeasured"
        );
    }
}
