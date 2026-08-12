//! Killable local Layer 1 helper boundary.
//!
//! Local weights never belong to the GUI process. This provider owns only a
//! child-process contract injected by the selected power-user runtime. The
//! concrete Qwen/Parakeet runner remains outside the app and outside this
//! crate; no model library is linked here and no failed helper can fall back to
//! in-process Whisper.
//!
//! Process exit is the reclaim authority. [`LocalHelperLifecycle::Stopped`]
//! after a session means the child was waited and reported exited — dropping a
//! handle or sending a shutdown request is not enough.

use super::events::{AsrErrorKind, AsrSessionEvent};
use super::provider::{AsrSessionProvider, RefinerMode, SessionInput};

/// Observable local-helper lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalHelperLifecycle {
    /// No child is owned. Initial state and confirmed post-exit state.
    Stopped,
    /// A child is being spawned and its session is being opened.
    Starting,
    /// The child accepted the session and can consume PCM.
    Ready,
    /// Shutdown was requested and the provider is waiting for process exit.
    Cooling,
}

/// Proof returned only after the operating system reports process exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalHelperExit {
    /// PID observed from the child handle.
    pub pid: u32,
    /// True only after a successful wait/reap.
    pub exited: bool,
}

/// One spawned helper process.
///
/// Implementations own IPC and their bounded shutdown deadline. Both
/// [`Self::wait_for_exit`] and [`Self::kill_and_wait`] must reap the process;
/// returning `exited: false` is treated as a transport failure, never reclaim.
pub trait LocalHelperProcess: Send {
    /// Operating-system process identifier.
    fn pid(&self) -> u32;

    /// Open one provider-compatible ASR session in the child.
    fn start(&mut self, input: &SessionInput) -> Result<(), AsrErrorKind>;

    /// Forward one PCM chunk without blocking the Apple capture lane.
    fn push_audio(&mut self, samples: &[f32]) -> Result<(), AsrErrorKind>;

    /// Drain typed events already available from the child. Never blocks.
    fn drain(&mut self) -> Vec<AsrSessionEvent>;

    /// Ask the child to finish its current session and exit.
    fn request_shutdown(&mut self) -> Result<(), AsrErrorKind>;

    /// Wait within the implementation's bounded graceful-exit deadline.
    fn wait_for_exit(&mut self) -> Result<LocalHelperExit, AsrErrorKind>;

    /// Kill, then wait and reap. This is the final reclaim backstop.
    fn kill_and_wait(&mut self) -> Result<LocalHelperExit, AsrErrorKind>;
}

/// Injected process factory.
///
/// The stock app has no default implementation. A power-user runtime must make
/// an explicit model/download decision and inject its launcher.
pub trait LocalHelperLauncher: Send {
    /// Spawn one helper process with no weights in the caller process.
    fn spawn(&mut self) -> Result<Box<dyn LocalHelperProcess>, AsrErrorKind>;
}

/// Provider-compatible owner of a killable local helper.
pub struct LocalHelperAsrSession {
    launcher: Box<dyn LocalHelperLauncher>,
    child: Option<Box<dyn LocalHelperProcess>>,
    lifecycle: LocalHelperLifecycle,
    transitions: Vec<LocalHelperLifecycle>,
    last_exit: Option<LocalHelperExit>,
    ever_opened: bool,
}

impl LocalHelperAsrSession {
    /// Build a stopped provider around an explicit launcher.
    pub fn new(launcher: Box<dyn LocalHelperLauncher>) -> Self {
        Self {
            launcher,
            child: None,
            lifecycle: LocalHelperLifecycle::Stopped,
            transitions: vec![LocalHelperLifecycle::Stopped],
            last_exit: None,
            ever_opened: false,
        }
    }

    /// Current lifecycle state.
    pub fn lifecycle(&self) -> LocalHelperLifecycle {
        self.lifecycle
    }

    /// Exact transition history, including the initial stopped state.
    pub fn transitions(&self) -> &[LocalHelperLifecycle] {
        &self.transitions
    }

    /// PID of the currently owned child, if any.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.as_ref().map(|child| child.pid())
    }

    /// Last confirmed process-exit proof.
    pub fn last_exit(&self) -> Option<LocalHelperExit> {
        self.last_exit
    }

    fn transition(&mut self, next: LocalHelperLifecycle) {
        self.lifecycle = next;
        self.transitions.push(next);
    }

    /// Reclaim `child`, preferring graceful exit and always falling back to a
    /// kill+wait when graceful shutdown is refused or unconfirmed.
    fn reclaim_child(child: &mut dyn LocalHelperProcess) -> Result<LocalHelperExit, AsrErrorKind> {
        let graceful = child
            .request_shutdown()
            .and_then(|()| child.wait_for_exit());
        match graceful {
            Ok(proof) if proof.pid != 0 && proof.exited => Ok(proof),
            Ok(_) | Err(_) => {
                let proof = child.kill_and_wait()?;
                if proof.pid == 0 || !proof.exited {
                    return Err(AsrErrorKind::Transport);
                }
                Ok(proof)
            }
        }
    }

    fn stop_owned_child(&mut self) -> Result<(), AsrErrorKind> {
        let Some(mut child) = self.child.take() else {
            self.transition(LocalHelperLifecycle::Stopped);
            return Ok(());
        };
        match Self::reclaim_child(child.as_mut()) {
            Ok(proof) => {
                self.last_exit = Some(proof);
                self.transition(LocalHelperLifecycle::Stopped);
                Ok(())
            }
            Err(kind) => {
                // We no longer claim Stopped: process exit was not proven.
                self.child = Some(child);
                Err(kind)
            }
        }
    }
}

impl AsrSessionProvider for LocalHelperAsrSession {
    fn mode(&self) -> RefinerMode {
        RefinerMode::LocalHelper
    }

    fn open(&mut self, input: &SessionInput) -> Result<(), AsrErrorKind> {
        if self.lifecycle != LocalHelperLifecycle::Stopped || self.ever_opened {
            return Err(AsrErrorKind::Protocol);
        }
        self.ever_opened = true;
        self.transition(LocalHelperLifecycle::Starting);

        let mut child = match self.launcher.spawn() {
            Ok(child) if child.pid() != 0 => child,
            Ok(mut child) => {
                let _ = Self::reclaim_child(child.as_mut());
                self.transition(LocalHelperLifecycle::Stopped);
                return Err(AsrErrorKind::Protocol);
            }
            Err(kind) => {
                self.transition(LocalHelperLifecycle::Stopped);
                return Err(kind);
            }
        };

        if let Err(kind) = child.start(input) {
            let reclaim = Self::reclaim_child(child.as_mut());
            if let Ok(proof) = reclaim {
                self.last_exit = Some(proof);
                self.transition(LocalHelperLifecycle::Stopped);
                return Err(kind);
            }
            self.child = Some(child);
            return Err(AsrErrorKind::Transport);
        }

        self.child = Some(child);
        self.transition(LocalHelperLifecycle::Ready);
        Ok(())
    }

    fn push_audio(&mut self, samples: &[f32]) -> Result<(), AsrErrorKind> {
        if self.lifecycle != LocalHelperLifecycle::Ready {
            return Err(AsrErrorKind::Protocol);
        }
        let result = self
            .child
            .as_mut()
            .ok_or(AsrErrorKind::Protocol)?
            .push_audio(samples);
        if let Err(kind) = result {
            self.transition(LocalHelperLifecycle::Cooling);
            self.stop_owned_child()?;
            return Err(kind);
        }
        Ok(())
    }

    fn drain(&mut self) -> Vec<AsrSessionEvent> {
        if self.lifecycle != LocalHelperLifecycle::Ready {
            return Vec::new();
        }
        self.child
            .as_mut()
            .map_or_else(Vec::new, |child| child.drain())
    }

    fn close(&mut self) -> Result<(), AsrErrorKind> {
        if self.lifecycle != LocalHelperLifecycle::Ready {
            return Err(AsrErrorKind::Protocol);
        }
        self.transition(LocalHelperLifecycle::Cooling);
        self.stop_owned_child()
    }
}

impl Drop for LocalHelperAsrSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take()
            && let Ok(proof) = Self::reclaim_child(child.as_mut())
        {
            self.last_exit = Some(proof);
            self.lifecycle = LocalHelperLifecycle::Stopped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingLauncher;

    impl LocalHelperLauncher for FailingLauncher {
        fn spawn(&mut self) -> Result<Box<dyn LocalHelperProcess>, AsrErrorKind> {
            Err(AsrErrorKind::Transport)
        }
    }

    fn input() -> SessionInput {
        SessionInput {
            session_id: super::super::SessionId::new("local-helper-test").expect("session id"),
            locale: Some("pl-PL".to_string()),
            sample_rate: 16_000,
        }
    }

    #[test]
    #[serial_test::serial]
    fn spawn_failure_returns_to_stopped_without_model_fallback() {
        let whisper_before = crate::stt::whisper::singleton::test_init_calls()
            + crate::stt::whisper::singleton::test_load_calls();
        let provider = LocalHelperAsrSession::new(Box::new(FailingLauncher));
        let mut lane = super::super::RecorderLayer1Lane::open(
            super::super::Layer1Decision::Armed(Box::new(provider)),
            &input(),
        );

        assert_eq!(
            lane.state(),
            super::super::Layer1LaneState::Degraded(super::super::Layer1DegradeReason::OpenFailed(
                AsrErrorKind::Transport
            ))
        );
        assert_eq!(lane.refiner_mode(), RefinerMode::Off);
        assert!(lane.stop().finals().is_empty());
        assert_eq!(
            crate::stt::whisper::singleton::test_init_calls()
                + crate::stt::whisper::singleton::test_load_calls(),
            whisper_before,
            "a failed local helper must remain Apple + lexicon, never initialize in-process Whisper"
        );
    }
}
