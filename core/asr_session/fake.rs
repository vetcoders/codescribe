//! A deterministic Layer 1 provider that talks to nothing.
//!
//! The real providers (a gateway websocket, a killable local helper) are later
//! cuts. Their tests will need something that produces the *shape* of a live
//! session — lifecycle faults, partials landing before finals, a trailing usage
//! record — without a socket, a model, a thread, or a clock. This is that
//! something.
//!
//! Everything it does is a pure function of the script it was built with and
//! the calls it received. There is no timing, no randomness, and no I/O, so a
//! test that passes here passes on a loaded machine too.

use std::collections::VecDeque;

use super::events::{AsrErrorKind, AsrSessionEvent, AudioRange, SessionId, UsageEvent};
use super::provider::{AsrSessionProvider, RefinerMode, SessionInput};

/// Utterance id the fake stamps on session-scoped records (its closing usage
/// event), which describe the whole session rather than one speech unit.
const SESSION_SCOPE_UTTERANCE: u64 = 0;

/// Where a fake session is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Constructed, not yet opened.
    Idle,
    /// Open and accepting audio.
    Open,
    /// Closed; no further audio accepted.
    Closed,
}

/// A scripted, in-memory [`AsrSessionProvider`].
#[derive(Debug)]
pub struct FakeAsrSessionProvider {
    /// Mode this fake claims to implement.
    mode: RefinerMode,
    /// Events still waiting to be released, in script order.
    script: VecDeque<AsrSessionEvent>,
    /// Events released and not yet drained.
    ready: Vec<AsrSessionEvent>,
    /// Lifecycle position.
    state: State,
    /// Session identity captured at open.
    session_id: Option<SessionId>,
    /// Sample rate captured at open, floored at 1 to keep the clock finite.
    sample_rate: u32,
    /// Total samples pushed, the fake's only notion of time.
    pushed_samples: u64,
    /// Exclusive end of the last recorded commit, on the capture clock.
    commit_cursor: u64,
    /// Capture spans waiting for the next unstamped final.
    pending_commits: VecDeque<AudioRange>,
    /// Highest sequence released so far, so the closing usage event stays
    /// monotonic whatever the script did.
    highest_sequence: Option<u64>,
    /// When set, every `push_audio` fails with this kind.
    push_failure: Option<AsrErrorKind>,
}

impl FakeAsrSessionProvider {
    /// Build a fake that will release `script` one event per pushed chunk.
    pub fn with_script(mode: RefinerMode, script: Vec<AsrSessionEvent>) -> Self {
        Self {
            mode,
            script: script.into(),
            ready: Vec::new(),
            state: State::Idle,
            session_id: None,
            sample_rate: 1,
            pushed_samples: 0,
            commit_cursor: 0,
            pending_commits: VecDeque::new(),
            highest_sequence: None,
            push_failure: None,
        }
    }

    /// Build a fake with no scripted transcript events.
    pub fn new(mode: RefinerMode) -> Self {
        Self::with_script(mode, Vec::new())
    }

    /// Make every `push_audio` fail with `kind` — the degradation harness.
    pub fn failing_pushes(mut self, kind: AsrErrorKind) -> Self {
        self.push_failure = Some(kind);
        self
    }

    /// Session seconds derived from pushed audio.
    pub fn pushed_secs(&self) -> f32 {
        self.pushed_samples as f32 / self.sample_rate as f32
    }

    /// Whether the script has been fully released.
    pub fn script_drained(&self) -> bool {
        self.script.is_empty()
    }

    /// Move one scripted event to the ready queue, tracking its sequence.
    fn release_one(&mut self) {
        if let Some(event) = self.script.pop_front() {
            let sequence = event.sequence_id();
            self.highest_sequence = Some(match self.highest_sequence {
                Some(previous) => previous.max(sequence),
                None => sequence,
            });
            self.ready.push(event);
        }
    }

    /// Sequence to stamp on the fake's own closing usage event.
    fn next_sequence(&self) -> u64 {
        self.highest_sequence.map_or(0, |highest| highest + 1)
    }

    /// Remember `[commit_cursor, commit_sample)` on the capture clock.
    fn record_commit(&mut self, commit_sample: u64) -> Result<(), AsrErrorKind> {
        if commit_sample < self.commit_cursor || commit_sample > self.pushed_samples {
            return Err(AsrErrorKind::Protocol);
        }
        if commit_sample == self.commit_cursor {
            return Ok(());
        }
        let range =
            AudioRange::from_capture_samples(self.commit_cursor, commit_sample, self.sample_rate)
                .ok_or(AsrErrorKind::Protocol)?;
        self.pending_commits.push_back(range);
        self.commit_cursor = commit_sample;
        Ok(())
    }

    /// Fill unstamped finals from the commit queue, oldest first.
    fn stamp_ready(&mut self, events: &mut [AsrSessionEvent]) {
        for event in events {
            if let AsrSessionEvent::Final(transcript) = event
                && transcript.range.is_none()
                && let Some(range) = self.pending_commits.pop_front()
            {
                transcript.range = Some(range);
            }
        }
    }
}

impl AsrSessionProvider for FakeAsrSessionProvider {
    /// Mode this fake was built for.
    fn mode(&self) -> RefinerMode {
        self.mode
    }

    /// Open once; a second open is a protocol fault, not a panic.
    fn open(&mut self, input: &SessionInput) -> Result<(), AsrErrorKind> {
        if self.state != State::Idle {
            return Err(AsrErrorKind::Protocol);
        }
        self.session_id = Some(input.session_id.clone());
        self.sample_rate = input.sample_rate.max(1);
        self.state = State::Open;
        Ok(())
    }

    /// Accept a chunk and release the next scripted event.
    fn push_audio(&mut self, samples: &[f32]) -> Result<(), AsrErrorKind> {
        if self.state != State::Open {
            return Err(AsrErrorKind::Protocol);
        }
        if let Some(kind) = self.push_failure {
            return Err(kind);
        }
        self.pushed_samples += samples.len() as u64;
        self.release_one();
        Ok(())
    }

    /// Hand over everything released so far.
    fn drain(&mut self) -> Vec<AsrSessionEvent> {
        let mut ready = std::mem::take(&mut self.ready);
        self.stamp_ready(&mut ready);
        ready
    }

    /// Record a capture-clock commit. The next unstamped final takes it.
    fn commit(&mut self, commit_sample: u64) -> Result<(), AsrErrorKind> {
        if self.state != State::Open {
            return Err(AsrErrorKind::Protocol);
        }
        self.record_commit(commit_sample)
    }

    /// Close, flushing the rest of the script and a usage record behind it.
    fn close(&mut self) -> Result<(), AsrErrorKind> {
        if self.state != State::Open {
            return Err(AsrErrorKind::Protocol);
        }
        if self.commit_cursor > 0 && self.pushed_samples > self.commit_cursor {
            self.record_commit(self.pushed_samples)?;
        }
        while !self.script.is_empty() {
            self.release_one();
        }
        if let Some(session_id) = self.session_id.clone() {
            self.ready.push(AsrSessionEvent::Usage(UsageEvent {
                session_id,
                utterance_id: SESSION_SCOPE_UTTERANCE,
                sequence_id: self.next_sequence(),
                audio_secs: self.pushed_secs(),
                billable_units: None,
            }));
        }
        self.state = State::Closed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_commit_stamps_the_next_unstamped_final_on_the_capture_clock() {
        let session_id = SessionId::new("fake-commit").expect("session id");
        let script = vec![AsrSessionEvent::Final(
            super::super::events::TranscriptEvent {
                session_id: session_id.clone(),
                utterance_id: 1,
                sequence_id: 1,
                text: "halo".to_string(),
                range: None,
            },
        )];
        let mut provider = FakeAsrSessionProvider::with_script(RefinerMode::CloudSession, script);
        provider
            .open(&SessionInput {
                session_id,
                locale: None,
                sample_rate: 48_000,
            })
            .expect("open");
        provider
            .push_audio(&vec![0.0; 384_000])
            .expect("capture samples");
        provider.commit(384_000).expect("commit");
        let events = provider.drain();
        let AsrSessionEvent::Final(event) = &events[0] else {
            panic!("expected a final");
        };
        let range = event.range.expect("commit stamp");
        assert_eq!(range.start_secs(), 0.0);
        assert_eq!(range.end_secs(), 8.0);
    }
}
