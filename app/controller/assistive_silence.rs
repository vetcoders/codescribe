//! A hands-free assistive take ends after `agent_take_voiceless_end_sec`
//! (default 30 s, Settings › Built-in Agent) without voice.
//!
//! A false start of the Agent take once recorded a room for 55 minutes and
//! kept the microphone live (W2-09, 2026-10-26). The take now ends on its own
//! after that limit without measured voice, through the same controller stop
//! a Stop click uses, and that end delivers nothing: the text stays in the
//! take archive. Only the take's own capture is released; an agent channel on
//! the same microphone stays open. `toggle_silence_sec` still only closes
//! utterance epochs.

use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use super::speech_clock::SpeechClock;
use super::{CaptureStopOutcome, RecordingController, STOPPING_SUFFIX};
use crate::presentation::emitter::SpeechActivityObserver;

/// How often the watch reads the take's speech clock.
const ASSISTIVE_SILENCE_POLL: Duration = Duration::from_millis(500);

/// The speech clock of one hands-free assistive take.
#[derive(Debug)]
pub(crate) struct AssistiveSilenceWatch {
    take_id: String,
    opened_at: SystemTime,
    clock: Arc<StdMutex<SpeechClock>>,
    /// Voiceless limit frozen from the take's runtime snapshot.
    voiceless_end_secs: u64,
    /// This watch ended the take; its stop then archives only.
    ended: bool,
    /// A failed stop was logged for this take.
    stop_failure_logged: bool,
}

/// What one poll found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssistiveSilencePoll {
    /// The take is live and has heard voice recently enough.
    Open,
    /// The watch stopped the take.
    Ended,
    /// The take is gone or is already stopping for another reason.
    Gone,
}

impl RecordingController {
    /// Arm the watch for a hands-free assistive take, or clear it for any
    /// other take. The returned observer feeds the take's emitter.
    pub(super) fn arm_assistive_silence(
        &self,
        take_id: &str,
        armed: bool,
        voiceless_end_secs: u64,
    ) -> Option<SpeechActivityObserver> {
        let mut slot = self
            .assistive_silence
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !armed {
            *slot = None;
            return None;
        }
        let clock = Arc::new(StdMutex::new(SpeechClock::default()));
        *slot = Some(AssistiveSilenceWatch {
            take_id: take_id.to_owned(),
            opened_at: SystemTime::now(),
            clock: Arc::clone(&clock),
            voiceless_end_secs,
            ended: false,
            stop_failure_logged: false,
        });
        Some(Arc::new(move |evidence| {
            clock
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .observe(evidence, SystemTime::now());
        }))
    }

    /// Poll the armed take until it ends. Nothing is spawned for a take the
    /// watch was not armed for.
    pub(super) fn watch_assistive_silence(self: &Arc<Self>, take_id: String) {
        if !self.assistive_silence_armed_for(&take_id) {
            return;
        }
        let controller = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ASSISTIVE_SILENCE_POLL).await;
                let playback = crate::audio::tts_duck::channel_capture_should_drop();
                match controller
                    .poll_assistive_silence(&take_id, SystemTime::now(), playback)
                    .await
                {
                    AssistiveSilencePoll::Open => {}
                    AssistiveSilencePoll::Ended | AssistiveSilencePoll::Gone => break,
                }
            }
        });
    }

    fn assistive_silence_armed_for(&self, take_id: &str) -> bool {
        self.assistive_silence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|watch| watch.take_id == take_id)
    }

    fn mark_assistive_silence_end(&self, take_id: &str, ended: bool) {
        if let Some(watch) = self
            .assistive_silence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .filter(|watch| watch.take_id == take_id)
        {
            watch.ended = ended;
        }
    }

    /// One read of the take's speech clock. `playback` is whether agent speech
    /// owns the speaker now; that time is not silence. When the take has heard
    /// no voice for its `voiceless_end_secs`, it is stopped by name through
    /// `stop_take_without_voice`, which marks the stop archive-only in the
    /// same settlement-slot section that registers it.
    pub(super) async fn poll_assistive_silence(
        self: &Arc<Self>,
        take_id: &str,
        now: SystemTime,
        playback: bool,
    ) -> AssistiveSilencePoll {
        if self.session_id.read().await.as_deref() != Some(take_id) {
            return AssistiveSilencePoll::Gone;
        }
        let secs = {
            let slot = self
                .assistive_silence
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(watch) = slot.as_ref().filter(|watch| watch.take_id == take_id) else {
                return AssistiveSilencePoll::Gone;
            };
            let mut clock = watch.clock.lock().unwrap_or_else(PoisonError::into_inner);
            if playback {
                clock.note_agent_playback(now);
            }
            if !clock.no_voice_for(watch.opened_at, now, watch.voiceless_end_secs) {
                return AssistiveSilencePoll::Open;
            }
            watch.voiceless_end_secs
        };
        info!(
            take = take_id,
            secs, "assistive take ends: no voice; its text stays in the take archive"
        );
        match self.stop_take_without_voice(take_id).await {
            Ok(CaptureStopOutcome::Stopped | CaptureStopOutcome::Pending) => {
                AssistiveSilencePoll::Ended
            }
            Ok(CaptureStopOutcome::AlreadyStopping) => {
                // A stop of this take is already registered: an explicit Stop
                // that got there first and delivers as usual, or this watch's
                // own stop from an earlier read that failed and stays in the
                // slot. Either way the watch ends here.
                AssistiveSilencePoll::Gone
            }
            Ok(
                outcome @ (CaptureStopOutcome::ForeignCapture | CaptureStopOutcome::NoLiveCapture),
            ) => {
                self.mark_assistive_silence_end(take_id, false);
                info!(
                    take = take_id,
                    ?outcome,
                    "assistive silence end found no take"
                );
                AssistiveSilencePoll::Gone
            }
            Ok(CaptureStopOutcome::AdmissionUnavailable) => {
                self.mark_assistive_silence_end(take_id, false);
                AssistiveSilencePoll::Open
            }
            Err(error) => {
                self.mark_assistive_silence_end(take_id, false);
                if self.first_stop_failure(take_id) {
                    warn!(%error, take = take_id, "assistive silence end could not stop the take");
                }
                AssistiveSilencePoll::Open
            }
        }
    }

    /// The named stop of [`RecordingController::stop_capture_if_owned`], with
    /// the archive-only mark set in the same settlement-slot section that
    /// registers the stop. A Stop of this take that registered first keeps
    /// its delivery: the mark is never set on it. The slot admission here
    /// repeats the one in `stop_capture_if_owned`; a change to either
    /// belongs in both until one function owns it.
    async fn stop_take_without_voice(
        self: &Arc<Self>,
        take_id: &str,
    ) -> anyhow::Result<CaptureStopOutcome> {
        let result = {
            let Ok(mut slot) = self.capture_settlement.try_lock() else {
                return Ok(CaptureStopOutcome::AdmissionUnavailable);
            };
            if slot.as_ref().is_some_and(|op| op.capture_id == take_id) {
                return Ok(CaptureStopOutcome::AlreadyStopping);
            }
            if slot
                .as_ref()
                .is_some_and(|op| !op.task.is_finished() || op.result.borrow().is_none())
            {
                return Ok(CaptureStopOutcome::AdmissionUnavailable);
            }
            self.mark_assistive_silence_end(take_id, true);
            self.register_capture_stop(&mut slot, take_id)
        };
        Self::await_capture_stop(result).await
    }

    /// True once per take: a stop that keeps failing is logged once, not on
    /// every poll.
    fn first_stop_failure(&self, take_id: &str) -> bool {
        self.assistive_silence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .filter(|watch| watch.take_id == take_id)
            .is_some_and(|watch| !std::mem::replace(&mut watch.stop_failure_logged, true))
    }

    /// Whether `take_id` was ended by silence; its stop then archives only.
    pub(super) fn take_ended_by_silence(&self, take_id: Option<&str>) -> bool {
        let Some(take_id) = take_id.map(|id| id.strip_suffix(STOPPING_SUFFIX).unwrap_or(id)) else {
            return false;
        };
        self.assistive_silence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|watch| watch.ended && watch.take_id == take_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::speech_clock::{VOICELESS_CLOSE_SECS, measured_clock_edge};
    use super::super::*;
    use super::*;
    use crate::audio::streaming_recorder::CaptureSubscriberKind;

    async fn start_assistive_take(controller: &Arc<RecordingController>) -> (String, SystemTime) {
        controller
            .handle_hotkey_event(HotkeyInput {
                key_type: HotkeyType::Toggle,
                action: HotkeyAction::Press,
                assistive: true,
                hold_mode: HoldMode::Raw,
                force_raw: false,
                force_ai: false,
            })
            .await
            .expect("the assistive start resolves");
        assert_eq!(controller.current_state().await, State::RecToggle);
        let take = controller
            .session_id
            .read()
            .await
            .clone()
            .expect("the take owns the session slot");
        let opened = controller
            .assistive_silence
            .lock()
            .unwrap()
            .as_ref()
            .filter(|watch| watch.take_id == take)
            .map(|watch| watch.opened_at)
            .expect("a hands-free assistive take arms the watch");
        (take, opened)
    }

    fn clock_of(controller: &RecordingController) -> Arc<StdMutex<SpeechClock>> {
        Arc::clone(
            &controller
                .assistive_silence
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .clock,
        )
    }

    fn after(opened: SystemTime, secs: u64) -> SystemTime {
        opened + Duration::from_secs(secs)
    }

    #[tokio::test]
    async fn a_take_without_voice_ends_after_thirty_seconds_and_releases_the_capture() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (take, opened) = start_assistive_take(&controller).await;

        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 29), false)
                .await,
            AssistiveSilencePoll::Open
        );
        assert_eq!(controller.current_state().await, State::RecToggle);

        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 30), false)
                .await,
            AssistiveSilencePoll::Ended
        );
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.session_id.read().await.is_none());
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 31), false)
                .await,
            AssistiveSilencePoll::Gone,
            "an ended take is not stopped twice"
        );
    }

    #[tokio::test]
    async fn speech_then_thirty_seconds_of_silence_end_the_take() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (take, opened) = start_assistive_take(&controller).await;
        let clock = clock_of(&controller);
        clock
            .lock()
            .unwrap()
            .observe(&measured_clock_edge(1, 16_000, true), after(opened, 1));
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 120), false)
                .await,
            AssistiveSilencePoll::Open,
            "speech without its end holds the take"
        );
        clock.lock().unwrap().observe(
            &measured_clock_edge(2, 1_920_000, false),
            after(opened, 121),
        );
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 150), false)
                .await,
            AssistiveSilencePoll::Open
        );
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 151), false)
                .await,
            AssistiveSilencePoll::Ended
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    async fn agent_playback_during_the_take_is_not_silence() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (take, opened) = start_assistive_take(&controller).await;
        for second in [10, 20, 28] {
            assert_eq!(
                controller
                    .poll_assistive_silence(&take, after(opened, second), true)
                    .await,
                AssistiveSilencePoll::Open
            );
        }
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 57), false)
                .await,
            AssistiveSilencePoll::Open,
            "the thirty seconds restart at the last playback"
        );
        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 58), false)
                .await,
            AssistiveSilencePoll::Ended
        );
    }

    /// The silence end releases only the take: a channel subscriber on the
    /// same capture survives it and keeps receiving.
    #[tokio::test]
    async fn the_silence_end_leaves_an_open_channel_on_the_microphone() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (channel_tx, _channel_rx) = tokio::sync::mpsc::channel::<Vec<f32>>(8);
        let channel = controller
            .recorder
            .lock()
            .await
            .as_mut()
            .expect("recorder fixture")
            .acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx);
        let (take, opened) = start_assistive_take(&controller).await;

        assert_eq!(
            controller
                .poll_assistive_silence(&take, after(opened, 30), false)
                .await,
            AssistiveSilencePoll::Ended
        );
        assert_eq!(controller.current_state().await, State::Idle);
        let mut guard = controller.recorder.lock().await;
        let recorder = guard.as_mut().expect("recorder fixture");
        assert!(recorder.has_non_take_subscriber());
        assert!(recorder.release_capture_subscriber(channel));
    }

    /// A take ended by silence never reaches route selection. The falsifier
    /// uses a paste mode under which the same words would go to a paste sink:
    /// after a silence end the sink is never called, while a stop of an
    /// unmarked take with the same words reaches it once.
    #[tokio::test]
    async fn a_take_ended_by_silence_is_archived_and_never_routed() {
        let controller = RecordingController::new_without_keychain();
        let config = Config {
            paste_mode: PasteMode::Safe,
            transcription_overlay_enabled: true,
            quick_notes_enabled: false,
            ..Config::default()
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = |calls: &Arc<AtomicUsize>| {
            let seen = Arc::clone(calls);
            move |route: DeliveryRoute, _: String, _: Option<String>| async move {
                assert_eq!(route, DeliveryRoute::ClipboardPaste);
                seen.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("test sink"))
            }
        };
        let silent = "take-silent";
        controller.arm_assistive_silence(silent, true, VOICELESS_CLOSE_SECS);
        controller.mark_assistive_silence_end(silent, true);
        let disposition = controller
            .deliver_stop_transcript_with_sink(
                Some(&format!("{silent}{STOPPING_SUFFIX}")),
                "words heard in the room",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                sink(&calls),
            )
            .await
            .expect("an archived take is a settled delivery");
        assert_eq!(disposition, TranscriptDelivery::EndedWithoutVoice);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a silence end reaches no sink"
        );
        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::EndedWithoutVoice
        );

        let stopped = "take-stopped";
        controller.arm_assistive_silence(stopped, true, VOICELESS_CLOSE_SECS);
        let _ = controller
            .deliver_stop_transcript_with_sink(
                Some(stopped),
                "words heard in the room",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                sink(&calls),
            )
            .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an explicit Stop is routed"
        );
    }

    #[tokio::test]
    async fn only_a_hands_free_assistive_take_is_watched() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let admitted = controller
            .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
            .await
            .expect("a dictation toggle resolves");
        let CaptureAdmission::Admitted(take) = admitted else {
            panic!("the dictation toggle starts");
        };
        assert!(!controller.assistive_silence_armed_for(&take));
        controller.reset().await;

        let admitted = controller
            .start_toggle_recording(true, CaptureTurnIntent::SingleTurn)
            .await
            .expect("a composer turn resolves");
        let CaptureAdmission::Admitted(take) = admitted else {
            panic!("the composer turn starts");
        };
        assert!(
            !controller.assistive_silence_armed_for(&take),
            "the composer ends its own turn explicitly"
        );
    }

    /// A Stop of the take that registered before the silence end keeps its
    /// delivery: the silence end joins it and never sets the archive mark.
    #[tokio::test]
    async fn a_stop_registered_first_is_never_marked_archive_only() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (take, _) = start_assistive_take(&controller).await;
        {
            let mut slot = controller.capture_settlement.lock().unwrap();
            let _ = controller.register_capture_stop(&mut slot, &take);
        }
        assert_eq!(
            controller.stop_take_without_voice(&take).await.unwrap(),
            CaptureStopOutcome::AlreadyStopping
        );
        assert!(!controller.take_ended_by_silence(Some(&take)));
        assert_eq!(
            controller.stop_capture_if_owned(&take).await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[test]
    fn a_failing_stop_is_logged_once_per_take() {
        let controller = RecordingController::new_without_keychain();
        controller.arm_assistive_silence("take-a", true, VOICELESS_CLOSE_SECS);
        assert!(controller.first_stop_failure("take-a"));
        assert!(!controller.first_stop_failure("take-a"));
        assert!(
            !controller.first_stop_failure("take-b"),
            "only the armed take"
        );
        controller.arm_assistive_silence("take-b", true, VOICELESS_CLOSE_SECS);
        assert!(controller.first_stop_failure("take-b"));
    }
}
