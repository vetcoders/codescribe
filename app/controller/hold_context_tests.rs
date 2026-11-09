pub use super::*;

#[cfg(test)]
mod hold_context_tests {
    use super::*;

    fn bucket(root: &std::path::Path) -> ContextBucket {
        ContextBucket::for_codescribe_data_dir(root)
    }

    fn assert_only_marker(events: &[EngineEvent], expected: &str, position: usize) {
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            EngineEvent::ContextMarker { position: at, label }
                if *at == position && label == expected
        ));
    }

    #[test]
    fn hold_with_no_selection_ignores_existing_clipboard_image() {
        let root = tempfile::tempdir().unwrap();
        let mut bucket = bucket(root.path());
        let clipboard_reads = std::cell::Cell::new(0);
        let (context, events) = RecordingController::record_captured_context(
            &mut bucket,
            0,
            false,
            CapturedAssistiveContext::default,
            || {
                clipboard_reads.set(clipboard_reads.get() + 1);
                Some(b"existing clipboard image".to_vec())
            },
        )
        .unwrap();
        assert_eq!(clipboard_reads.get(), 0);
        assert!(context.selected_text.is_none());
        assert!(bucket.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn hold_keeps_selected_text_marker() {
        let root = tempfile::tempdir().unwrap();
        let mut bucket = bucket(root.path());
        let (_, events) = RecordingController::record_captured_context(
            &mut bucket,
            7,
            false,
            || CapturedAssistiveContext {
                context: AssistiveContext {
                    selected_text: Some("selected words".into()),
                    frontmost_app: None,
                },
                image_png: None,
            },
            || panic!("hold must not read the clipboard image"),
        )
        .unwrap();
        assert!(!bucket.is_empty());
        assert_eq!(bucket.image_count(), 0);
        assert_only_marker(&events, "{selection_1}", 7);
    }

    #[test]
    fn hold_keeps_image_produced_by_selection_capture() {
        let root = tempfile::tempdir().unwrap();
        let mut bucket = bucket(root.path());
        let (_, events) = RecordingController::record_captured_context(
            &mut bucket,
            7,
            false,
            || CapturedAssistiveContext {
                context: AssistiveContext::default(),
                image_png: Some(b"selected image".to_vec()),
            },
            || panic!("captured selection image must not trigger clipboard fallback"),
        )
        .unwrap();
        assert_eq!(bucket.image_count(), 1);
        assert_only_marker(&events, "{image_1}", 7);
    }

    #[test]
    fn assistive_combo_keeps_clipboard_image_when_selection_is_empty() {
        let root = tempfile::tempdir().unwrap();
        let mut bucket = bucket(root.path());
        let (_, events) = RecordingController::record_captured_context(
            &mut bucket,
            7,
            true,
            CapturedAssistiveContext::default,
            || Some(b"clipboard image".to_vec()),
        )
        .unwrap();
        assert_eq!(bucket.image_count(), 1);
        assert_only_marker(&events, "{image_1}", 7);
    }
}

#[cfg(test)]
// Terminal settlement projects into the process-wide tray and badge stores.
#[serial_test::serial]
mod terminal_delivery_target_falsifiers {
    use super::*;
    use crate::audio::streaming_recorder::CaptureSubscriberKind;

    #[tokio::test]
    async fn failed_stop_publishes_failure_instead_of_completed() {
        use crate::presentation::transcript_bus::{
            CleanTranscriptEvent, TranscriptMode, TranscriptProjectionPhase, TranscriptSession,
        };
        for failure in ["terminal seal refused", "recorder failed after drain"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            let controller = RecordingController::new_without_keychain();
            let bus = TranscriptBus::open_at(
                TranscriptSession {
                    session_id: "failed-stop".to_string(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: false,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                path.clone(),
                None,
            )
            .unwrap();
            bus.publish_started();
            *controller.active_transcript_bus.write().await = Some(Arc::new(bus));

            controller
                .reset_finished_recording_state(&Err(anyhow::anyhow!(failure)))
                .await;

            let rows: Vec<CleanTranscriptEvent> = std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[1].end_reason,
                Some(TranscriptSessionEndReason::TranscriptionFailed)
            );
            assert_eq!(rows[1].phase, TranscriptProjectionPhase::Error);
            assert_eq!(controller.current_state().await, State::Idle);
        }
    }

    /// A completed take returns the recorder to Idle before the overlay Insert
    /// click. Its foreign caret must survive that transition, while explicit
    /// recovery still clears the latch so a later take cannot inherit it.
    #[tokio::test]
    async fn completed_reset_preserves_target_until_explicit_recovery() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("Ghostty".to_string());

        controller
            .reset_finished_recording_state(&Ok(ProcessRecordingOutcome::default()))
            .await;

        assert_eq!(controller.current_state().await, State::Idle);
        assert_eq!(
            controller.paste_target_app_name().await.as_deref(),
            Some("Ghostty")
        );

        controller.reset().await;
        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// Hold release goes through `finish_recording`, which first clears the
    /// delayed-start task. The pre-overlay app latched at hotkey press is the
    /// take's delivery target, not the start task's scratch: the overlay Insert
    /// click that follows the sealed canvas still needs it. Falsifier for the
    /// `target_app=None frontmost_app=Some("vc-terminal")` lines in the log.
    #[tokio::test]
    async fn hold_release_after_live_start_preserves_target_latch() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        *controller.pre_overlay_frontmost_app.write().await = Some("vc-terminal".to_string());
        let started = tokio::spawn(async {});
        while !started.is_finished() {
            tokio::task::yield_now().await;
        }
        *controller.hold_start_task.lock().await = Some(started);

        controller
            .finish_recording()
            .await
            .expect("finish at Idle is an ignored race, not an error");

        assert_eq!(
            controller.paste_target_app_name().await.as_deref(),
            Some("vc-terminal"),
            "a finished hold start is a live take; its delivery target survives release"
        );
    }

    /// Releasing the hold key before the start delay elapsed means no take ever
    /// existed, so the latch captured for it must go with the cancelled task.
    #[tokio::test]
    async fn quick_hold_release_before_start_clears_target_latch() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("vc-terminal".to_string());
        let pending = tokio::spawn(std::future::pending::<()>());
        *controller.hold_start_task.lock().await = Some(pending);

        controller.cancel_pending_hold_start().await;

        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// A start that never became a take has no terminal canvas and therefore
    /// must not leave a target behind for an unrelated later Insert click.
    /// Its cleanup also writes the process-wide tray/badge projection, so it
    /// runs serially with the tests that assert that projection.
    #[tokio::test]
    #[serial_test::serial]
    async fn failed_start_clears_target_latch() {
        let controller = RecordingController::new_without_keychain();
        *controller.pre_overlay_frontmost_app.write().await = Some("Ghostty".to_string());

        controller
            .reset_session_after_start_failure("delivery target falsifier")
            .await;

        assert!(controller.paste_target_app_name().await.is_none());
    }

    /// One take, one destination. A composer turn belongs to the Agent draft of
    /// the thread that opened it; posting a synthetic paste as well is how a
    /// voice note lands in whatever window happened to be frontmost.
    #[tokio::test]
    async fn a_composer_turn_is_owed_to_the_composer_and_to_nothing_else() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-composer"),
                "words for the draft",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                false,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// The wrapper is created after route selection and is handed to both OS
    /// transports as one immutable payload. A deferred reuse must not wrap it
    /// a second time.
    #[tokio::test]
    async fn stop_clipboard_and_deferred_routes_receive_one_tagged_payload() {
        for (paste_mode, force_ai, expected_route) in [
            (PasteMode::Safe, false, DeliveryRoute::ClipboardPaste),
            (PasteMode::Comfort, false, DeliveryRoute::ClipboardPaste),
            (PasteMode::Safe, true, DeliveryRoute::ClipboardPaste),
        ] {
            let controller = RecordingController::new_without_keychain();
            controller.delivery_tagger.begin("dictation", "pl");
            let config = Config {
                transcript_tagging_enabled: true,
                transcript_tag_template: "<codescribe mode=\"{mode}\">{text}</codescribe>".into(),
                paste_mode,
                transcription_overlay_enabled: true,
                quick_notes_enabled: false,
                ..Config::default()
            };

            controller
                .deliver_stop_transcript_with_sink(
                    Some(if force_ai {
                        "tagged-format"
                    } else {
                        "tagged-dictation"
                    }),
                    "{selection_1} working  {image_1}  text {name}",
                    (false, force_ai, CaptureTurnIntent::HandsFree, false),
                    &config,
                    |route, payload, _| async move {
                        assert_eq!(route, expected_route);
                        assert_eq!(
                            payload,
                            format!(
                                "<codescribe mode=\"{}\">working text {{name}}</codescribe>",
                                if force_ai { "format" } else { "dictation" }
                            )
                        );
                        assert_eq!(payload.matches("<codescribe").count(), 1);
                        Ok(OverlayPasteResult {
                            delivery: if route == DeliveryRoute::ClipboardPaste {
                                OverlayPasteDelivery::PasteRequested
                            } else {
                                OverlayPasteDelivery::DeferredInsertArmed
                            },
                            target_app_name: None,
                            frontmost_app_name: None,
                            deferred_insert_shortcut: None,
                            deferred_insert_failure: None,
                        })
                    },
                )
                .await
                .unwrap();
        }
    }

    /// Composer delivery gets its own sink payload while the reducer document
    /// remains the clean text passed into this boundary. The payload keeps the
    /// bucket markers the Agent resolves and never the outward transcript tag:
    /// dictation on the Agent's own path is not a paste into a foreign app.
    #[tokio::test]
    async fn composer_route_delivers_untagged_text_separate_from_working_text() {
        let controller = RecordingController::new_without_keychain();
        controller.delivery_tagger.begin("agent", "en");
        let config = Config {
            transcript_tagging_enabled: true,
            transcript_tag_template: "[{mode}|{lang}|{conf}] {text}".into(),
            ..Config::default()
        };

        controller
            .deliver_stop_transcript_with_sink(
                Some("composer-untagged"),
                "{selection_1} editable {image_1} draft",
                (true, false, CaptureTurnIntent::SingleTurn, false),
                &config,
                |_, _, _| async { panic!("composer route must not call an OS sink") },
            )
            .await
            .unwrap();

        assert_eq!(
            controller.composer_delivery_payload.read().await.as_ref(),
            Some(&(
                "composer-untagged".to_string(),
                "{selection_1} editable {image_1} draft".to_string()
            ))
        );
    }

    /// `ComposerPending` is an obligation, so an empty capture must not create
    /// one. Nothing was said; nothing is owed.
    #[tokio::test]
    async fn an_empty_composer_turn_creates_no_delivery_obligation() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-empty"),
                "   \n  ",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                false,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::Retained
        );
    }

    #[tokio::test]
    async fn empty_stop_canvas_cannot_claim_the_take_before_terminal_words_arrive() {
        let controller = RecordingController::new_without_keychain();
        let config = Config::default();
        let first = controller
            .deliver_stop_transcript_with_sink(
                Some("open-last-window"),
                "",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |_, _, _| async { panic!("empty canvas has no sink payload") },
            )
            .await;
        assert!(first.is_ok());
        let terminal = controller
            .deliver_stop_transcript_with_sink(
                Some("open-last-window"),
                "last words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |_, text, _| async move {
                    assert_eq!(text, "last words");
                    Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::PasteRequested,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    })
                },
            )
            .await;
        assert!(
            terminal.is_ok(),
            "terminal words were suppressed: {terminal:?}"
        );
    }

    #[tokio::test]
    async fn sink_admission_after_last_window_ignores_slow_tail_work() {
        let controller = RecordingController::new_without_keychain();
        let config = Config {
            paste_mode: PasteMode::Safe,
            ..Config::default()
        };
        let l1 = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(650)).await });
        let recovery = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(700)).await });
        let stop_start = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(55)).await;
        let last_window_close_ms = stop_start.elapsed().as_millis();
        controller
            .deliver_stop_transcript_with_sink(
                Some("timed-take"),
                "all committed words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |route, text, _| async move {
                    assert_eq!(route, DeliveryRoute::ClipboardPaste);
                    assert_eq!(text, "all committed words");
                    Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::PasteRequested,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    })
                },
            )
            .await
            .unwrap();
        let stop_to_sink_ms = stop_start.elapsed().as_millis();
        eprintln!("last_window_close_ms={last_window_close_ms} stop_to_sink_ms={stop_to_sink_ms}");
        assert!(stop_to_sink_ms < last_window_close_ms + 300);
        assert!(!l1.is_finished());
        assert!(!recovery.is_finished());
        l1.await.unwrap();
        recovery.await.unwrap();
    }

    /// A refused seal degrades the coverage claim, not the route. The committed
    /// words still belong to the composer that captured them.
    #[tokio::test]
    async fn a_refused_seal_keeps_the_composer_as_the_destination() {
        let controller = RecordingController::new_without_keychain();

        controller
            .deliver_stop_transcript(
                Some("take-refused"),
                "partial but real words",
                true,
                false,
                CaptureTurnIntent::SingleTurn,
                true,
            )
            .await
            .unwrap();

        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// The converse, which is the assertion that keeps the branch honest: a
    /// hands-free take is never owed to the composer.
    #[tokio::test]
    async fn a_hands_free_take_is_never_owed_to_the_composer() {
        let controller = RecordingController::new_without_keychain();

        let result = controller
            .deliver_stop_transcript_with_sink(
                Some("take-hands-free"),
                "dictated words",
                (false, false, CaptureTurnIntent::HandsFree, false),
                &controller.get_config().await,
                |_, _, _| async { Err(anyhow::anyhow!("injected sink refusal")) },
            )
            .await;

        if let Err(error) = result {
            assert!(error.is::<StopDeliveryFailure>());
        }

        assert_ne!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
    }

    /// A named stop refuses when a different capture owns the microphone, and
    /// says nothing is live when nothing is. Both are refusals, and they are
    /// different refusals: one must leave a foreign take running.
    #[tokio::test]
    async fn the_capture_gate_tells_a_foreign_take_apart_from_no_take() {
        let controller = RecordingController::new_without_keychain();

        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::NoLiveCapture
        );

        *controller.session_id.write().await = Some("theirs".to_string());
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::ForeignCapture
        );

        *controller.session_id.write().await = Some("mine".to_string());
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::Stopped
        );

        *controller.session_id.write().await = Some(format!("mine{STOPPING_SUFFIX}"));
        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::AlreadyStopping
        );
    }

    /// The hotkey and tray contract is unchanged: an unnamed stop still stops
    /// whatever is live, including a take it did not open.
    #[tokio::test]
    async fn an_unnamed_stop_keeps_its_unconditional_contract() {
        let controller = RecordingController::new_without_keychain();
        *controller.session_id.write().await = Some("someone-elses-take".to_string());

        assert_eq!(
            controller.capture_gate(None).await,
            CaptureStopOutcome::Stopped
        );
    }

    /// A conditional stop that loses the identity race must not fall through to
    /// the stop path. The foreign take keeps the microphone.
    #[tokio::test]
    async fn a_conditional_stop_for_a_foreign_take_does_not_run_the_stop_path() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::RecToggle).await;
        *controller.session_id.write().await = Some("theirs".to_string());

        let outcome = controller
            .stop_capture_if_owned("mine")
            .await
            .expect("a refusal is an outcome, not an error");

        assert_eq!(outcome, CaptureStopOutcome::ForeignCapture);
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("theirs"),
            "the foreign take's identity is untouched"
        );
    }

    /// Take admission requires "no active dictation take", never "no capture":
    /// a toggle start is admitted while a non-take subscriber (the agent
    /// channel) holds the capture open, and a second dictation take is still
    /// refused.
    #[tokio::test]
    async fn take_admission_ignores_a_non_take_capture_subscriber() {
        let controller = RecordingController::new_without_keychain();
        let (channel_tx, _channel_rx) = mpsc::channel::<Vec<f32>>(8);
        let channel = {
            let mut guard = controller.recorder.lock().await;
            guard
                .as_mut()
                .expect("recorder fixture")
                .acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx)
        };

        let admitted = controller
            .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
            .await
            .expect("a take start resolves to an admission, not an error");
        assert!(
            matches!(admitted, CaptureAdmission::Admitted(_)),
            "a take must start while only a channel subscribes to the capture"
        );
        assert_eq!(controller.current_state().await, State::RecToggle);

        let second = controller
            .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
            .await
            .expect("a refusal is an admission outcome, not an error");
        assert!(
            matches!(second, CaptureAdmission::NotAdmitted),
            "one dictation take at a time"
        );

        // The channel subscription survives the whole take gesture untouched.
        controller.reset().await;
        let mut guard = controller.recorder.lock().await;
        let recorder = guard.as_mut().expect("recorder fixture");
        assert!(recorder.has_non_take_subscriber());
        assert!(recorder.release_capture_subscriber(channel));
        assert_eq!(recorder.capture_subscriber_count(), 0);
    }

    /// The gate reads take identity, never capture existence: a capture kept
    /// open by a non-take subscriber registers no take, so stops report "no
    /// live capture" and the channel's capture is left untouched.
    #[tokio::test]
    async fn capture_gate_reads_take_identity_not_capture_existence() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let (channel_tx, _channel_rx) = mpsc::channel::<Vec<f32>>(8);
        controller
            .recorder
            .lock()
            .await
            .as_mut()
            .expect("recorder fixture")
            .acquire_capture_subscriber(CaptureSubscriberKind::Channel, channel_tx);

        assert_eq!(
            controller.capture_gate(Some("mine")).await,
            CaptureStopOutcome::NoLiveCapture,
            "an open capture without a take is not a stop target"
        );
        assert_eq!(
            controller
                .stop_current_capture()
                .await
                .expect("a refusal is an outcome, not an error"),
            CaptureStopOutcome::NoLiveCapture
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }

    /// Regression: with no channel subscriber, the hold→toggle→hold gesture
    /// ladder keeps the pre-refcount contract — the same state transitions,
    /// the same stop receipts, and an empty session slot whenever Idle.
    #[tokio::test]
    async fn hold_toggle_hold_without_a_channel_keeps_the_pre_refcount_contract() {
        let controller = Arc::new(RecordingController::new_without_keychain());

        for leg in 0..2 {
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
                .expect("hold down resolves");
            // The delayed hold start owns the IDLE → REC_HOLD transition.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while controller.current_state().await != State::RecHold {
                assert!(
                    std::time::Instant::now() < deadline,
                    "hold leg {leg}: start did not reach REC_HOLD"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            controller
                .handle_hotkey_event(HotkeyInput {
                    key_type: HotkeyType::Hold,
                    action: HotkeyAction::Up,
                    assistive: false,
                    hold_mode: HoldMode::Raw,
                    force_raw: false,
                    force_ai: false,
                })
                .await
                .expect("hold up resolves");
            assert_eq!(
                controller.current_state().await,
                State::Idle,
                "hold leg {leg}: stop settles to IDLE"
            );
            assert!(
                controller.session_id.read().await.is_none(),
                "hold leg {leg}: Idle owns no take slot"
            );
            assert_eq!(
                controller.capture_gate(None).await,
                CaptureStopOutcome::Stopped,
                "hold leg {leg}: the unnamed-stop receipt contract is unchanged"
            );

            if leg == 0 {
                // Assistive toggle: its stop routes through the plain
                // finish path (adjudication requires non-assistive), the same
                // headless-proven settlement the fixture stops exercise.
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
                    .expect("toggle press resolves");
                assert_eq!(
                    controller.current_state().await,
                    State::RecToggle,
                    "toggle leg: start reaches REC_TOGGLE"
                );
                let toggle_take = controller
                    .session_id
                    .read()
                    .await
                    .clone()
                    .expect("toggle leg: the take owns the session slot");
                let stopped = controller
                    .stop_capture_if_owned(&toggle_take)
                    .await
                    .expect("toggle leg: stop resolves");
                assert_eq!(
                    stopped,
                    CaptureStopOutcome::Stopped,
                    "toggle leg: named stop receipt"
                );
                assert_eq!(controller.current_state().await, State::Idle);
                assert!(controller.session_id.read().await.is_none());
            }
        }

        // No subscriber leaked through three takes: the registry is empty and
        // the next take would open a fresh capture, exactly as before.
        let guard = controller.recorder.lock().await;
        let recorder = guard.as_ref().expect("recorder fixture");
        assert_eq!(recorder.capture_subscriber_count(), 0);
        assert!(!recorder.has_capture_subscribers());
    }
}

/// Authored W2 falsifiers, UNRUN. The ledger/emitter/Bus/reset are real; only
/// capture input and receiver responses are synthetic. No process_recording
/// test shortcut is evidence for these terminal decisions.
#[cfg(test)]
#[serial_test::serial]
mod refusal_recovery_tests {
    #[tokio::test]
    async fn stop_light_plus_noop_is_not_reported_as_applied() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(false);
        let initial = stop_mutation(&mut take.ledger.lock().unwrap(), "Gotowy tekst.");
        take.emitter.on_event(&initial);
        let frozen = take.emitter.begin_stop_canvas().unwrap();
        assert_eq!(frozen.preview_only_words, 0);
        assert!(frozen.has_committed_document);
        let revision = frozen.revision;
        let before = take
            .ledger
            .lock()
            .unwrap()
            .manual_document_revisions()
            .len();
        let wait = StopCanvasWait {
            snapshot: Some(frozen.clone()),
            stop_final_wait_ms: 0,
            stop_final_timeout: false,
            live_finals_admitted: true,
            painted_at_stop: Some(frozen),
            preempted: false,
            armed_order: true,
        };
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let settled = take
            .controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| async move {
                    assert_eq!(text, "Gotowy tekst.");
                    Ok(TranscriptDelivery::SinkAccepted)
                },
            )
            .await
            .unwrap();
        assert_eq!(settled, TranscriptDelivery::SinkAccepted);
        assert_eq!(
            take.ledger
                .lock()
                .unwrap()
                .manual_document_revisions()
                .len(),
            before
        );
        assert_eq!(
            take.emitter.visible_canvas_snapshot().unwrap().revision,
            revision
        );
        let log = receipts.text();
        let lines = log
            .lines()
            .filter(|line| line.contains("stop canvas delivery settled"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1, "{log}");
        assert!(
            !lines[0].contains("light_plus=\"applied\""),
            "A no-op did not mint a Light+ receipt: {}",
            lines[0]
        );
    }

    use super::*;
    use crate::presentation::transcript_bus::{
        ProjectedSealCoverageReceipt, TranscriptBusEvidenceEvent, TranscriptProjectionPhase,
    };
    use codescribe_core::audio::capture_receipt::{
        AcousticAvailability, AcousticSpeechEvidence, CaptureEvidenceIdentity,
    };
    use codescribe_core::pipeline::acoustic_ledger::{
        AcousticEvidence, AcousticLedger, EnergyCalibration, ObservationIdentity,
        ObservationProducer, OccurrenceIdentity, SealRefusal,
    };
    use codescribe_core::pipeline::contracts::EventSink;
    use codescribe_core::stt::tail_provider::TailSampleRange;

    const TAKE: &str = "refusal-capture";
    const WORDS: &str = "Te słowa zostały.";

    #[derive(Clone, Default)]
    struct StopReceiptLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for StopReceiptLog {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl StopReceiptLog {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }

        fn subscribe(&self) -> tracing::subscriber::DefaultGuard {
            let writer = self.clone();
            tracing::subscriber::set_default(
                tracing_subscriber::fmt()
                    .without_time()
                    .with_ansi(false)
                    .with_writer(move || writer.clone())
                    .finish(),
            )
        }
    }

    fn stop_preview(text: &str) -> EngineEvent {
        use codescribe_core::pipeline::contracts::{
            UnadmittedAppleWord, UnadmittedAppleWordSource,
        };
        EngineEvent::UnadmittedAppleWords {
            revision: 1,
            closed_phrases: Default::default(),
            words: text
                .split_whitespace()
                .map(|word| UnadmittedAppleWord {
                    text: word.into(),
                    sample_start: 0,
                    sample_end: 16_000,
                    source: UnadmittedAppleWordSource::OpenPartial {
                        rev: 1,
                        phrase_id: 1,
                    },
                })
                .collect(),
        }
    }

    fn stop_closed_phrase(words: usize) -> EngineEvent {
        use codescribe_core::pipeline::contracts::{ApplePhraseOutcome, ClosedApplePhrase};
        EngineEvent::UnadmittedAppleWords {
            revision: 2,
            words: Vec::new(),
            closed_phrases: std::collections::BTreeMap::from([(
                1,
                ClosedApplePhrase {
                    arrival_index: 0,
                    outcomes: std::collections::BTreeMap::from([(
                        ApplePhraseOutcome::Admitted,
                        words,
                    )]),
                    was_untimed: false,
                },
            )]),
        }
    }

    // Synthetic PCM qualification; emitted only when the injected finish step
    // runs. The reducer starts empty even though acoustic evidence already exists.
    fn stop_mutation(ledger: &mut AcousticLedger, text: &str) -> EngineEvent {
        let occurrence = OccurrenceIdentity::new(TAKE, 7, 0, 16_000);
        let calibration = EnergyCalibration {
            version: "stop-synthetic".into(),
            min_energy_integral: 1.0,
            min_valley_samples: 1,
        };
        let evidence = AcousticEvidence {
            occurrence: occurrence.clone(),
            duration_ms: 1_000.0,
            energy_integral: 10.0,
            mean_rms_dbfs: -12.0,
            peak_dbfs: -3.0,
            vad_open_sample: Some(0),
            vad_close_sample: Some(16_000),
            evidence_calibration_version: calibration.version.clone(),
        };
        assert!(ledger.qualify(&evidence, &calibration).is_qualified());
        let observation = ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence);
        let receipt = ledger.admit(&observation, text);
        EngineEvent::LedgerMutation {
            observation,
            label: text.into(),
            receipt,
        }
    }

    fn stop_whisper_mutation(ledger: &mut AcousticLedger, text: &str) -> EngineEvent {
        let observation = ObservationIdentity::new(
            ObservationProducer::Whisper,
            2,
            0,
            OccurrenceIdentity::new(TAKE, 7, 0, 16_000),
        );
        let receipt = ledger.admit(&observation, text);
        EngineEvent::LedgerMutation {
            observation,
            label: text.into(),
            receipt,
        }
    }

    async fn stop_sink(
        controller: &RecordingController,
        text: String,
        calls: &AtomicUsize,
    ) -> Result<TranscriptDelivery> {
        let config = Config {
            paste_mode: PasteMode::Safe,
            quick_notes_enabled: false,
            ..Config::default()
        };
        let expected = text.clone();
        controller
            .deliver_stop_transcript_with_sink(
                Some(TAKE),
                &text,
                (false, false, CaptureTurnIntent::HandsFree, false),
                &config,
                |route, payload, _| async move {
                    assert_eq!(route, DeliveryRoute::ClipboardPaste);
                    assert_eq!(payload, expected);
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(OverlayPasteResult {
                        delivery: OverlayPasteDelivery::PasteRequested,
                        target_app_name: None,
                        frontmost_app_name: None,
                        deferred_insert_shortcut: None,
                        deferred_insert_failure: None,
                    })
                },
            )
            .await
    }

    fn stop_live_final(text: &str) -> EngineEvent {
        EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: text.into(),
            raw_text: text.into(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: Vec::new(),
            vad_speech_pct: None,
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: Vec::new(),
        }
    }

    async fn check_late_final_delivery(
        final_text: Option<&str>,
        in_bound: bool,
        during_sink: bool,
    ) {
        let take = take(State::RecHold, false).await;
        let pipeline = RecordingController::build_recording_event_sink(
            Arc::new(Mutex::new(String::new())),
            RecordingEventSinkOptions {
                preview_deltas_enabled: false,
                sentence_pause_sec: 0.0,
                seal_ledger_enabled: true,
                apple_live_enabled: true,
                speech_observer: None,
            },
            take.controller.event_broadcast.clone(),
            Some(Arc::clone(&take.bus)),
            Some(Arc::clone(&take.ledger)),
            Arc::clone(&take.controller.delivery_tagger),
        );
        *take.controller.active_stop_receipt.write().await = Arc::clone(&pipeline.stop_receipt);
        pipeline.presentation.on_capture_opened(TAKE, 7);
        pipeline.presentation.set_literal_delivery(true);
        // Positive delivery begins with real committed PCM provenance. The
        // separate preview-only falsifiers continue to forbid preview paste.
        let initial = stop_mutation(&mut take.ledger.lock().unwrap(), "one two");
        pipeline.event_sink.on_event(&initial);
        let stopped_at = tokio::time::Instant::now();
        pipeline
            .stop_receipt
            .lock()
            .unwrap()
            .begin(Some(TAKE), stopped_at);
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let publish_final = || {
            if let Some(text) = final_text {
                let mutation = stop_whisper_mutation(&mut take.ledger.lock().unwrap(), text);
                pipeline.event_sink.on_event(&mutation);
                pipeline
                    .event_sink
                    .on_event(&stop_closed_phrase(text.split_whitespace().count()));
                pipeline.event_sink.on_event(&stop_live_final(text));
            }
        };
        let wait = await_live_finals_for_delivery(
            async {
                if in_bound {
                    tokio::time::sleep(STOP_FINAL_BOUND / 2).await;
                    publish_final();
                    true
                } else {
                    std::future::pending::<bool>().await
                }
            },
            || pipeline.presentation.visible_canvas_snapshot(),
            stopped_at,
            pipeline.presentation.visible_canvas_snapshot(),
            true,
            None,
        )
        .await;
        assert_eq!(wait.stop_final_timeout, !in_bound);
        let calls = AtomicUsize::new(0);
        let first = if wait.permits_first_delivery(Some(TAKE)) {
            Some(
                take.controller
                    .settle_frozen_canvas_at_stop(
                        Some(TAKE),
                        Some(&pipeline.presentation),
                        wait,
                        stopped_at.into_std(),
                        |text| {
                            let controller = &take.controller;
                            let calls = &calls;
                            let publish_final = &publish_final;
                            async move {
                                if during_sink {
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    publish_final();
                                }
                                stop_sink(controller, text, calls).await
                            }
                        },
                    )
                    .await
                    .unwrap(),
            )
        } else {
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "timeout cannot settle the existing prefix"
            );
            None
        };
        if !in_bound {
            tokio::time::sleep(Duration::from_secs(1)).await;
            publish_final();
        }
        let delivered = if let Some(text) = final_text {
            let canvas = pipeline.presentation.visible_canvas_snapshot().unwrap();
            assert_eq!(canvas.text, text);
            assert!(canvas.has_committed_document);
            assert_eq!(canvas.preview_only_words, 0);
            let delivered = deliver_terminal_unless_settled(first, || {
                stop_sink(&take.controller, canvas.text, &calls)
            })
            .await
            .unwrap();
            pipeline.event_sink.on_event(&stop_live_final(text));
            pipeline
                .event_sink
                .on_event(&stop_live_final("another final"));
            assert_eq!(
                pipeline
                    .presentation
                    .visible_canvas_snapshot()
                    .unwrap()
                    .text,
                text,
                "raw final observations cannot rewrite committed text"
            );
            delivered
        } else {
            assert!(first.is_none());
            TranscriptDelivery::Retained
        };
        deliver_terminal_unless_settled(Some(delivered), || async {
            panic!("terminal delivery attempted a second paste")
        })
        .await
        .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(final_text.is_some())
        );
        assert!(
            !receipts.text().contains("late_final_words_after_bound="),
            "a deferred first delivery is not a completed prefix paste"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn late_final_delivers_all_terminal_words_once_after_timeout() {
        check_late_final_delivery(Some("one two three four"), false, false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn late_final_with_identical_words_does_not_duplicate_delivery() {
        check_late_final_delivery(Some("one two"), false, false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn in_bound_final_emits_no_late_receipt() {
        check_late_final_delivery(Some("one two three four"), true, false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_without_a_final_never_pastes_a_committed_prefix() {
        check_late_final_delivery(None, false, false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn final_racing_the_sink_joins_the_settlement_once() {
        check_late_final_delivery(Some("one two three four"), true, true).await;
    }

    #[test]
    fn late_word_comparison_preserves_order_case_punctuation_and_multiplicity() {
        assert_eq!(
            late_final_missing_words("Iwo Iwo Iwo Iwo Iwo", "Iwo Iwo Iwo Iwo Iwo"),
            0
        );
        assert_eq!(
            late_final_missing_words("Iwo Iwo Iwo", "Iwo Iwo Iwo Iwo Iwo"),
            2
        );
        assert_eq!(
            late_final_missing_words("one two", "new one middle two tail"),
            3
        );
        assert_eq!(late_final_missing_words("one two", "two one"), 1);
        assert_eq!(late_final_missing_words("One two.", "one two"), 2);
        assert_eq!(
            late_final_missing_words(" one  {selection_1} two\n", "one\ttwo"),
            0
        );
        assert_eq!(late_final_missing_words("", "one two"), 2);
        assert_eq!(late_final_missing_words("one two", ""), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_copy_outcomes_remain_distinct() {
        // Inject the transport boundary, not clipboard or focus side effects.
        // Target copy/deferred outcomes require committed text; preemption retains it.
        for (outcome, frontmost, preempted, expected, receipt) in [
            (
                OverlayPasteDelivery::PasteRequested,
                Some("original-editor"),
                false,
                TranscriptDelivery::SinkAccepted,
                "pasted",
            ),
            (
                OverlayPasteDelivery::CopiedToClipboard,
                Some("changed-editor"),
                false,
                TranscriptDelivery::CopiedToClipboard,
                "copied",
            ),
            (
                OverlayPasteDelivery::CopiedToClipboard,
                None,
                false,
                TranscriptDelivery::CopiedToClipboard,
                "copied",
            ),
            (
                OverlayPasteDelivery::CopiedToClipboard,
                Some("original-editor"),
                true,
                TranscriptDelivery::Retained,
                "retained",
            ),
            (
                OverlayPasteDelivery::DeferredInsertArmed,
                Some("Codescribe"),
                false,
                TranscriptDelivery::DeferredInsertArmed,
                "deferred_insert",
            ),
        ] {
            let take = take(State::RecHold, false).await;
            take.emitter.on_capture_opened(TAKE, 7);
            take.emitter.set_literal_delivery(true);
            let initial = stop_mutation(&mut take.ledger.lock().unwrap(), "one two");
            take.emitter.on_event(&initial);
            let stopped_at = tokio::time::Instant::now();
            let stop_receipt = take.controller.active_stop_receipt.read().await.clone();
            stop_receipt.lock().unwrap().begin(Some(TAKE), stopped_at);
            let mut wait = await_live_finals_for_delivery(
                async { true },
                || take.emitter.visible_canvas_snapshot(),
                stopped_at,
                take.emitter.visible_canvas_snapshot(),
                true,
                None,
            )
            .await;
            wait.preempted = preempted;
            let receipts = StopReceiptLog::default();
            let _trace = receipts.subscribe();
            let calls = AtomicUsize::new(0);
            let settled = take
                .controller
                .settle_frozen_canvas_at_stop(
                    Some(TAKE),
                    Some(&take.emitter),
                    wait,
                    stopped_at.into_std(),
                    |text| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(text, "one two");
                        take.controller.finish_stop_delivery(
                            Ok(OverlayPasteResult {
                                delivery: outcome,
                                target_app_name: Some("original-editor".into()),
                                frontmost_app_name: frontmost.map(str::to_owned),
                                deferred_insert_shortcut: None,
                                deferred_insert_failure: None,
                            }),
                            false,
                        )
                    },
                )
                .await
                .unwrap();
            assert_eq!(settled, expected);
            assert_eq!(*take.controller.delivery_disposition.read().await, expected);
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(!preempted));
            let log = receipts.text();
            let settled_line = log
                .lines()
                .find(|line| line.contains("stop canvas delivery settled"))
                .expect("settled receipt");
            assert!(settled_line.contains(&format!("delivery=\"{receipt}\"")));
            stop_receipt
                .lock()
                .unwrap()
                .observe(&stop_live_final("one two three"));
            assert!(!receipts.text().contains("late_final_words_after_bound="));
            assert_eq!(
                deliver_terminal_unless_settled(Some(settled), || async {
                    panic!("settled delivery attempted again")
                })
                .await
                .unwrap(),
                expected
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_late_apple_receipt_uses_the_frozen_canvas() {
        use codescribe_core::pipeline::acoustic_ledger::NoAuthorityReason;

        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        let mutation = stop_mutation(&mut take.ledger.lock().unwrap(), "document");
        take.emitter.on_event(&mutation);
        let late_word = |start, end, request, text: &str| {
            let observation = ObservationIdentity::new(
                ObservationProducer::Apple,
                request,
                request,
                OccurrenceIdentity::new(TAKE, 7, start, end),
            );
            let receipt = take.ledger.lock().unwrap().keep_visible_unanchored(
                &observation,
                text,
                NoAuthorityReason::LateAppleWordNotCurrent,
            );
            take.emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: text.into(),
                receipt,
            });
        };
        late_word(2_000, 4_000, 2, "alpha");
        late_word(8_000, 10_000, 3, "beta");
        let observation = ObservationIdentity::new(
            ObservationProducer::Whisper,
            4,
            4,
            OccurrenceIdentity::new(TAKE, 7, 0, 16_000),
        );
        let receipt = take.ledger.lock().unwrap().admit_word_slots_for_tests(
            &observation,
            &[codescribe_core::pipeline::acoustic_ledger::WordPin::new(
                9_000, 10_000, "heard",
            )],
        );
        take.emitter.on_event(&EngineEvent::LedgerMutation {
            observation,
            label: String::new(),
            receipt,
        });
        let frozen = take.emitter.begin_stop_canvas().unwrap();
        assert_eq!(frozen.late_apple_word_counts(), (0, 0));
        assert!(!frozen.text.split_whitespace().any(|word| word == "alpha"));
        assert!(!frozen.text.split_whitespace().any(|word| word == "beta"));
        let expected_paste = frozen.text.clone();
        let wait = StopCanvasWait {
            snapshot: Some(frozen.clone()),
            stop_final_wait_ms: 0,
            stop_final_timeout: false,
            live_finals_admitted: true,
            painted_at_stop: Some(frozen),
            preempted: false,
            armed_order: true,
        };
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let calls = AtomicUsize::new(0);
        let settled = take
            .controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| {
                    assert_eq!(text, expected_paste);
                    // New evidence during delivery must not change this receipt.
                    late_word(12_000, 14_000, 5, "gamma");
                    assert_eq!(
                        take.emitter
                            .visible_canvas_snapshot()
                            .unwrap()
                            .late_apple_word_counts(),
                        (0, 0)
                    );
                    stop_sink(&take.controller, text, &calls)
                },
            )
            .await
            .unwrap();
        assert_eq!(settled, TranscriptDelivery::SinkAccepted);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(take.ledger.lock().unwrap().len(), 1);
        let log = receipts.text();
        let mut settled_lines = log
            .lines()
            .filter(|line| line.contains("stop canvas delivery settled"));
        let line = settled_lines.next().expect("settled receipt");
        assert!(settled_lines.next().is_none());
        assert!(!line.contains("late_apple_words_pasted="));
        assert!(!line.contains("late_apple_words_covered_by_slot="));
        assert!(line.contains("delivery=\"pasted\""));
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_receipt_without_late_apple_keeps_base_bytes() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        let initial = stop_mutation(&mut take.ledger.lock().unwrap(), "one two");
        take.emitter.on_event(&initial);
        let frozen = take.emitter.begin_stop_canvas().unwrap();
        assert_eq!(frozen.late_apple_word_counts(), (0, 0));
        let revision = frozen.revision;
        let wait = StopCanvasWait {
            snapshot: Some(frozen.clone()),
            stop_final_wait_ms: 2_000,
            stop_final_timeout: false,
            live_finals_admitted: true,
            painted_at_stop: Some(frozen),
            preempted: false,
            armed_order: true,
        };
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        take.controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| async move {
                    assert_eq!(text, "one two");
                    Ok(TranscriptDelivery::SinkAccepted)
                },
            )
            .await
            .unwrap();
        let log = receipts.text();
        let mut settled_lines = log
            .lines()
            .filter(|line| line.contains("stop canvas delivery settled"));
        let line = settled_lines.next().expect("settled receipt");
        assert!(settled_lines.next().is_none());
        // Pin field order and values for this committed fixture. Only the
        // wall-clock duration varies; preserve its captured value verbatim.
        let (prefix, fields) = line.split_once("stop_to_delivery_ms=").unwrap();
        let target = module_path!()
            .strip_suffix("::tests::refusal_recovery_tests")
            .unwrap();
        assert_eq!(
            prefix,
            format!(" INFO {target}: stop canvas delivery settled ")
        );
        let elapsed = fields.split_whitespace().next().unwrap();
        assert!(elapsed.parse::<u128>().is_ok());
        let expected = format!(
            concat!(
                "{}stop_to_delivery_ms={} delivery=\"pasted\" ",
                "stop_final_wait_ms=2000 stop_final_timeout=false live_finals_admitted=true ",
                "painted_words_at_stop=2 painted_words_at_snapshot=2 ",
                "superseded_by_partial_words=0 admitted_into_words=0 closed_by_final_words=0 ",
                "retained_as_evidence_words=0 untimed_final_phrases=0 ",
                "untimed_final_phrase_arrivals=Some([]) moved_to_pending_words=0 ",
                "moved_to_unmatched_words=0 untimed_final_words=0 covered_by_committed_words=0 ",
                "relabeled_in_place_words=0 reshaped_in_place_words=0 unaccounted=0 ",
                "armed_order=true light_plus=\"literal\" capture_epoch=7 reducer_revision={} ",
                "preview_only_words=0 preview_words_in_paste=0 paste_words=2 ",
                "paste_periods=0 paste_commas=0 paste_qe=0"
            ),
            prefix, elapsed, revision
        );
        assert_eq!(line.as_bytes(), expected.as_bytes());
        assert!(!line.contains("late_apple_words_"));
    }

    /// The production admission predicate is shared with the stop path. A
    /// timeout releases archive settlement, never a preview-text paste.
    #[tokio::test(start_paused = true)]
    async fn fn_release_timeout_defers_first_paste_until_terminal_words() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        take.emitter.on_event(&stop_preview("one two"));
        let wait = await_live_finals_for_delivery(
            std::future::pending::<bool>(),
            || take.emitter.visible_canvas_snapshot(),
            tokio::time::Instant::now(),
            take.emitter.visible_canvas_snapshot(),
            true,
            None,
        )
        .await;
        assert!(wait.stop_final_timeout);
        assert!(!wait.permits_first_delivery(Some(TAKE)));
        let calls = AtomicUsize::new(0);
        // No delivery claim is consumed while only the preview exists.
        let settled = if wait.permits_first_delivery(Some(TAKE)) {
            Some(
                stop_sink(&take.controller, wait.snapshot.unwrap().text, &calls)
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let mutation = stop_mutation(&mut take.ledger.lock().unwrap(), "one two three four");
        take.emitter.on_event(&mutation);
        take.emitter.on_event(&stop_closed_phrase(4));
        let canvas = take.emitter.visible_canvas_snapshot().unwrap();
        assert!(canvas.has_committed_document);
        assert_eq!(canvas.preview_only_words, 0);
        let text = canvas.text;
        assert_eq!(text, "one two three four");
        let delivered = deliver_terminal_unless_settled(settled, || {
            stop_sink(&take.controller, text.clone(), &calls)
        })
        .await
        .unwrap();
        deliver_terminal_unless_settled(Some(delivered), || async {
            panic!("terminal result attempted a second paste")
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn fn_release_ack_cannot_authorize_preview_or_a_foreign_take() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.on_event(&stop_preview("one two"));
        let mut wait = await_live_finals_for_delivery(
            async { true },
            || take.emitter.visible_canvas_snapshot(),
            tokio::time::Instant::now(),
            None,
            true,
            None,
        )
        .await;
        assert!(!wait.permits_first_delivery(Some(TAKE)));
        let mutation = stop_mutation(&mut take.ledger.lock().unwrap(), "one two three four");
        take.emitter.on_event(&mutation);
        take.emitter.on_event(&stop_closed_phrase(4));
        wait.snapshot = take.emitter.visible_canvas_snapshot();
        assert!(wait.permits_first_delivery(Some(TAKE)));
        assert!(!wait.permits_first_delivery(Some("next-take")));
        wait.preempted = true;
        assert!(!wait.permits_first_delivery(Some(TAKE)));
        wait.preempted = false;
        wait.live_finals_admitted = false;
        assert!(!wait.permits_first_delivery(Some(TAKE)));
    }

    /// First words arrive with a live final inside the stop bound. Delivery
    /// never waits for repair.
    #[tokio::test(start_paused = true)]
    async fn armed_ack_does_not_paste_before_an_in_bound_apple_final() {
        let take = take(State::RecToggle, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        let initial = stop_mutation(&mut take.ledger.lock().unwrap(), "last");
        take.emitter.on_event(&initial);
        let mutation =
            stop_whisper_mutation(&mut take.ledger.lock().unwrap(), "last complete words");
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        ack_tx.send(()).unwrap();
        ack_rx.await.unwrap();
        let terminal_tail = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(20)).await;
        });
        let final_at = STOP_FINAL_BOUND - Duration::from_millis(500);
        let wait = await_live_finals_for_delivery(
            async {
                tokio::time::sleep(final_at).await;
                take.emitter.on_event(&mutation);
                take.emitter.on_event(&stop_closed_phrase(3));
                take.emitter.on_event(&EngineEvent::PreviewDisposition {
                    superseded_through_rev: 1,
                    final_disposition:
                        codescribe_core::pipeline::contracts::PreviewFinalDisposition::Admitted,
                    refused_evidence: Vec::new(),
                });
                let at_ack = take.emitter.visible_canvas_snapshot().unwrap();
                assert!(at_ack.has_committed_document);
                assert_eq!(at_ack.text, "last complete words");
                true
            },
            || take.emitter.visible_canvas_snapshot(),
            tokio::time::Instant::now(),
            take.emitter.visible_canvas_snapshot(),
            true,
            None,
        )
        .await;
        assert_eq!(wait.stop_final_wait_ms, final_at.as_millis());
        assert_eq!(wait.snapshot.as_ref().unwrap().text, "last complete words");
        assert!(wait.live_finals_admitted);
        assert!(!wait.stop_final_timeout);
        let calls = AtomicUsize::new(0);
        let settled = take
            .controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| stop_sink(&take.controller, text, &calls),
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!terminal_tail.is_finished());
        assert!(
            receipts
                .text()
                .contains(&format!("stop_final_wait_ms={}", final_at.as_millis()))
        );
        assert!(receipts.text().contains("stop_final_timeout=false"));
        assert!(receipts.text().contains("paste_words=3"));
        terminal_tail.await.unwrap();
        deliver_terminal_unless_settled(Some(settled), || async {
            panic!("repair tail attempted short-take paste")
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Amendment 1 step 4, R4: speech evidence cannot authorize an empty sink
    /// payload, and words discovered by the terminal tail cannot reopen stop.
    #[tokio::test(start_paused = true)]
    async fn empty_after_the_stop_bound_is_retained_without_a_late_paste() {
        let take = take(State::RecToggle, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        let late = stop_mutation(&mut take.ledger.lock().unwrap(), "late words");
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let wait = await_live_finals_for_delivery(
            std::future::pending(),
            || take.emitter.visible_canvas_snapshot(),
            tokio::time::Instant::now(),
            take.emitter.visible_canvas_snapshot(),
            true,
            None,
        )
        .await;
        let bound_ms = STOP_FINAL_BOUND.as_millis();
        assert!((bound_ms..=bound_ms + 50).contains(&wait.stop_final_wait_ms));
        let settled = take
            .controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |_| async { panic!("empty stop called the sink") },
            )
            .await
            .unwrap();
        assert_eq!(settled, TranscriptDelivery::Retained);
        assert_eq!(
            *take.controller.delivery_disposition.read().await,
            TranscriptDelivery::Retained
        );
        let receipt = receipts.text();
        assert!(receipt.contains("stop_canvas_empty"));
        assert!(receipt.contains("qualified_occurrences=1"));
        assert!(receipt.contains("stop canvas delivery settled"));
        take.emitter.on_event(&late);
        assert!(
            !take
                .emitter
                .visible_canvas_snapshot()
                .unwrap()
                .text
                .is_empty()
        );
        deliver_terminal_unless_settled(Some(settled), || async {
            panic!("late words reopened empty stop")
        })
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn phrase_closure_alone_cannot_authorize_a_prefix_paste() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        take.emitter.on_event(&stop_preview("three visible words"));
        let at_stop = take.emitter.begin_stop_canvas().unwrap();
        let mutation = stop_mutation(&mut take.ledger.lock().unwrap(), "shorter");
        take.emitter.on_event(&mutation);
        take.emitter.on_event(&stop_closed_phrase(1));
        take.emitter.on_event(&EngineEvent::PreviewDisposition {
            superseded_through_rev: 1,
            final_disposition:
                codescribe_core::pipeline::contracts::PreviewFinalDisposition::Admitted,
            refused_evidence: Vec::new(),
        });
        let wait = await_live_finals_for_delivery(
            async { true },
            || take.emitter.finish_stop_canvas(),
            tokio::time::Instant::now(),
            Some(at_stop),
            true,
            None,
        )
        .await;
        let calls = AtomicUsize::new(0);
        let settled = take
            .controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| stop_sink(&take.controller, text, &calls),
            )
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "closure is not delivery lineage"
        );
        assert_eq!(settled, TranscriptDelivery::Retained);
    }

    // A shorter final accounts for stopped words through the phrase closure.
    #[tokio::test(start_paused = true)]
    async fn shorter_revision_accounts_for_each_admitted_visible_word() {
        let take = take(State::RecHold, false).await;
        take.emitter.on_capture_opened(TAKE, 7);
        take.emitter.set_literal_delivery(true);
        take.emitter.on_event(&stop_preview("three visible words"));
        let at_stop = take.emitter.begin_stop_canvas().unwrap();
        let mutation = stop_mutation(&mut take.ledger.lock().unwrap(), "shorter");
        take.emitter.on_event(&mutation);
        take.emitter.on_event(&stop_closed_phrase(1));
        take.emitter.on_event(&EngineEvent::PreviewDisposition {
            superseded_through_rev: 1,
            final_disposition:
                codescribe_core::pipeline::contracts::PreviewFinalDisposition::Admitted,
            refused_evidence: Vec::new(),
        });
        let receipts = StopReceiptLog::default();
        let _trace = receipts.subscribe();
        let wait = await_live_finals_for_delivery(
            async { true },
            || take.emitter.finish_stop_canvas(),
            tokio::time::Instant::now(),
            Some(at_stop),
            true,
            None,
        )
        .await;
        let calls = AtomicUsize::new(0);
        take.controller
            .settle_frozen_canvas_at_stop(
                Some(TAKE),
                Some(&take.emitter),
                wait,
                std::time::Instant::now(),
                |text| stop_sink(&take.controller, text, &calls),
            )
            .await
            .unwrap();
        assert!(receipts.text().contains("stop_paste_lost_visible_words"));
        assert!(receipts.text().contains("painted_words_at_stop=3"));
        assert!(receipts.text().contains("paste_words=1"));
        assert!(
            receipts
                .text()
                .contains("closed_by_final phrase=1 outcomes=admitted=1")
        );
        assert!(receipts.text().contains("closed_by_final_words=3"));
        assert!(receipts.text().contains("superseded_by_partial_words=0"));
        assert!(receipts.text().contains("covered_by_committed_words=0"));
        assert!(receipts.text().contains("unaccounted=0"));
        assert!(!receipts.text().contains("stop_paste_visible_word_defect"));
    }

    #[tokio::test(start_paused = true)]
    async fn live_final_bound_starts_at_stop_not_at_the_wait() {
        let stopped_at = tokio::time::Instant::now();
        tokio::time::sleep(STOP_FINAL_BOUND / 2).await;
        let wait = await_live_finals_for_delivery(
            std::future::pending(),
            || None,
            stopped_at,
            None,
            true,
            None,
        )
        .await;
        assert_eq!(wait.stop_final_wait_ms, STOP_FINAL_BOUND.as_millis());
        assert!(wait.stop_final_timeout);
        assert!(!wait.live_finals_admitted);
    }

    #[tokio::test(start_paused = true)]
    async fn lost_live_final_channel_releases_stop_immediately() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        drop(sender);
        let wait = await_live_finals_for_delivery(
            async { receiver.await.is_ok() },
            || None,
            tokio::time::Instant::now(),
            None,
            true,
            None,
        )
        .await;
        assert_eq!(wait.stop_final_wait_ms, 0);
        assert!(!wait.stop_final_timeout);
        assert!(!wait.live_finals_admitted);
    }

    /// Both races run through the live wait, settlement, retirement and new
    /// admission. The tail delay and OS sink are the only injected effects.
    ///
    /// The preemption lane keeps this test's preempt aimed at its own stop:
    /// the stop-paste slot is process-global, so a foreign test's registration
    /// could otherwise eat the preempt while this test's stop is preempted by
    /// a foreign start.
    #[tokio::test(start_paused = true)]
    async fn next_take_preempts_pending_or_same_tick_final_once() {
        let _preemption_lane = crate::test_env::StopPastePreemptionTestLane::acquire();
        for final_same_tick in [false, true] {
            let take = take(State::Busy, false).await;
            take.emitter.on_capture_opened(TAKE, 7);
            take.emitter.set_literal_delivery(true);
            take.emitter
                .on_event(&stop_preview("painted pending words"));
            let before = take.emitter.begin_stop_canvas();
            let control = StopPasteWaitControl::default();
            let receipts = StopReceiptLog::default();
            let _trace = receipts.subscribe();
            let stopped_at = tokio::time::Instant::now();
            let request_start = async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                control.preempted.store(true, Ordering::SeqCst);
                control.wake.notify_one();
            };
            let waiting = await_live_finals_for_delivery(
                async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if !final_same_tick {
                        std::future::pending::<()>().await;
                    }
                    true
                },
                || take.emitter.finish_stop_canvas(),
                stopped_at,
                before,
                true,
                Some(&control),
            );
            // Poll the new-start request first when both timers become ready.
            let (_, wait) = tokio::join!(biased; request_start, waiting);
            assert!(wait.preempted);
            assert!(wait.stop_final_timeout);
            assert_eq!(wait.stop_final_wait_ms, 2_000);
            let calls = AtomicUsize::new(0);
            let serial = take.controller.serial_lock.lock().await;
            let settled = take
                .controller
                .settle_frozen_canvas_at_stop(
                    Some(TAKE),
                    Some(&take.emitter),
                    wait,
                    std::time::Instant::now(),
                    |text| stop_sink(&take.controller, text, &calls),
                )
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(settled, TranscriptDelivery::Retained);
            take.emitter.retire_presentation();
            let late_ui = AtomicUsize::new(0);
            take.emitter.with_active_presentation(|| {
                late_ui.fetch_add(1, Ordering::SeqCst);
            });
            assert_eq!(late_ui.load(Ordering::SeqCst), 0);
            // Pending archive ownership is observable but does not serialize
            // capture admission. No old tail may acquire controller state.
            let tail = tokio::spawn(async {
                tokio::time::sleep(Duration::from_secs(20)).await;
            });
            take.controller
                .closed_capture_tails
                .lock()
                .unwrap()
                .push(tail);
            take.controller
                .reset_finished_recording_state(&Ok(ProcessRecordingOutcome::default()))
                .await;
            drop(serial);
            let admitted = take
                .controller
                .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
                .await
                .unwrap();
            assert!(matches!(admitted, CaptureAdmission::Admitted(_)));
            assert_eq!(take.controller.current_state().await, State::RecToggle);
            assert_eq!(stopped_at.elapsed(), Duration::from_secs(2));
            assert!(!take.controller.closed_capture_tails.lock().unwrap()[0].is_finished());
            assert_eq!(
                receipts
                    .text()
                    .matches("stop_paste_preempted_by_next_take")
                    .count(),
                1
            );
            deliver_terminal_unless_settled(Some(settled), || async {
                panic!("retired take attempted delivery inside the next take")
            })
            .await
            .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(settled, TranscriptDelivery::Retained);
            let tails = std::mem::take(&mut *take.controller.closed_capture_tails.lock().unwrap());
            for task in tails {
                task.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn retired_pipeline_drains_text_without_ipc_or_new_take_paint() {
        let buffer = Arc::new(Mutex::new(String::new()));
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let (events, mut receiver) = broadcast::channel(32);
        let pipeline = RecordingController::build_recording_event_sink(
            Arc::clone(&buffer),
            RecordingEventSinkOptions {
                preview_deltas_enabled: false,
                sentence_pause_sec: 0.7,
                seal_ledger_enabled: true,
                apple_live_enabled: true,
                speech_observer: None,
            },
            events,
            None,
            Some(Arc::clone(&ledger)),
            Arc::default(),
        );
        pipeline.event_sink.on_capture_opened(TAKE, 7);
        // Literal lane: the drained buffer holds the exact label, with no Light+ floor.
        pipeline.presentation.set_literal_delivery(true);
        while receiver.try_recv().is_ok() {}
        pipeline.presentation.retire_presentation();
        let final_event = stop_mutation(&mut ledger.lock().unwrap(), "late archive words");
        pipeline.event_sink.on_event(&final_event);
        pipeline.event_sink.on_event(&EngineEvent::Warning {
            code: "retired-test".into(),
            message: "must remain off the next take UI".into(),
        });
        assert!(pipeline.presentation.wait_paint_published().await);
        assert_eq!(*buffer.lock().await, "late archive words");
        assert!(matches!(
            receiver.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    /// Both stop sites use the same terminal settlement on their Ok and Err
    /// arms. Authenticated terminal refusal still reaches history, never paste.
    #[tokio::test]
    async fn terminal_path_never_pastes_after_stop_paste() {
        for state in [State::RecHold, State::RecToggle] {
            for disposition in [
                TranscriptDelivery::SinkAccepted,
                TranscriptDelivery::Retained,
            ] {
                let take = take(state, true).await;
                let ok = deliver_terminal_unless_settled(Some(disposition), || async {
                    panic!("successful terminal path pasted twice")
                })
                .await
                .unwrap();
                assert_eq!(ok, disposition);
                let refused = take
                    .controller
                    .process_terminal_stop_error(
                        anyhow::Error::new(take.refusal.clone()),
                        |_| async {
                            deliver_terminal_unless_settled(Some(disposition), || async {
                                panic!("refused terminal path pasted twice")
                            })
                            .await
                        },
                    )
                    .await
                    .unwrap();
                assert!(refused.refusal.is_some());
            }
        }
        // Tie the injected production seam to both actual recorder stop sites;
        // process_recording's cfg(test) shortcut must not stand in for this.
        let source = include_str!("mod.rs");
        for (start, end) in [
            (
                "    async fn stop_toggle_and_adjudicate_inner(",
                "    fn publish_live_serving_verdict(",
            ),
            ("    async fn process_recording(", "    pub async fn reset("),
        ] {
            let body = source
                .split_once(start)
                .unwrap()
                .1
                .split_once(end)
                .unwrap()
                .0;
            assert_eq!(
                body.matches("deliver_terminal_unless_settled(settled,")
                    .count(),
                2
            );
            assert_eq!(body.matches(".deliver_frozen_canvas_at_stop(").count(), 1);
        }
    }

    fn fixture_refusal(
        receipt: &codescribe_core::pipeline::acoustic_ledger::SealCoverageReceipt,
    ) -> codescribe_core::pipeline::acoustic_ledger::TerminalFinalityRefusal {
        let mut ledger = AcousticLedger::new();
        assert!(ledger.record_seal_coverage(receipt.clone()));
        ledger
            .terminal_finality(&receipt.session_id, receipt.capture_epoch)
            .into_refusal()
            .expect("fixture has no issued terminal seal")
    }

    struct Take {
        controller: RecordingController,
        bus: Arc<TranscriptBus>,
        ledger: Arc<std::sync::Mutex<AcousticLedger>>,
        emitter: PresentationEmitter,
        refusal: TerminalSealRefused,
        events: broadcast::Receiver<IpcEvent>,
        dir: tempfile::TempDir,
    }

    async fn take(state: State, words: bool) -> Take {
        take_with(state, words, true).await
    }

    /// `observed` selects which refusal the ledger issues: measured uncovered
    /// speech, or no authenticated acoustic measurement at all. Both must reach
    /// the same recovery path and keep the same committed words.
    async fn take_with(state: State, words: bool, observed: bool) -> Take {
        let controller = RecordingController::new_without_keychain();
        controller.set_state(state).await;
        *controller.session_id.write().await = Some(format!("{TAKE}:stopping"));
        *controller.pre_overlay_frontmost_app.write().await = Some("original-editor".into());
        let events = controller.subscribe_events();
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: TAKE.into(),
                    mode: TranscriptMode::Agent,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                dir.path().join("bus.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::clone(&bus));
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let mut emitter = PresentationEmitter::new_with_authority(
            Arc::new(Mutex::new(String::new())),
            None,
            None,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            None,
        );
        if words {
            let occurrence = OccurrenceIdentity::new(TAKE, 7, 0, 16_000);
            let calibration = EnergyCalibration {
                version: "refusal-synthetic-test".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            };
            let evidence = AcousticEvidence {
                occurrence: occurrence.clone(),
                duration_ms: 1_000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(0),
                vad_close_sample: Some(16_000),
                evidence_calibration_version: calibration.version.clone(),
            };
            let observation =
                ObservationIdentity::new(ObservationProducer::Apple, 1, 0, occurrence);
            let receipt = {
                let mut ledger = ledger.lock().unwrap();
                assert!(ledger.qualify(&evidence, &calibration).is_qualified());
                ledger.admit(&observation, WORDS)
            };
            emitter.on_event(&EngineEvent::LedgerMutation {
                observation,
                label: WORDS.into(),
                receipt,
            });
        }
        let receipt = {
            let mut ledger = ledger.lock().unwrap();
            let identity = CaptureEvidenceIdentity::new(TAKE, 7);
            let evidence = if observed {
                AcousticSpeechEvidence::measured(
                    identity,
                    "capture_energy",
                    AcousticAvailability::Observed {
                        observed_samples: 48_000,
                    },
                    vec![TailSampleRange {
                        session: TAKE.into(),
                        capture_epoch: 7,
                        sample_start: 0,
                        sample_end: 48_000,
                    }],
                )
            } else {
                AcousticSpeechEvidence::unavailable(
                    identity,
                    "capture_energy",
                    AcousticAvailability::NotObserved,
                )
            };
            let coverage = ledger.assess_seal_coverage(TAKE, 7, &evidence, 8_000);
            assert!(!coverage.status.is_complete());
            assert!(ledger.record_seal_coverage(coverage.clone()));
            assert_eq!(
                ledger.seal_terminal(TAKE, 7),
                Err(SealRefusal::CoverageIncomplete)
            );
            coverage
        };
        emitter.on_event(&EngineEvent::SealCoverage {
            receipt: receipt.clone(),
            comparison: None,
        });
        emitter.finish().await;
        let audio = dir.path().join("refused.wav");
        std::fs::write(&audio, b"synthetic retained WAV witness").unwrap();
        Take {
            controller,
            bus,
            ledger,
            emitter,
            events,
            dir,
            refusal: TerminalSealRefused {
                finality: fixture_refusal(&receipt),
                audio_path: Some(audio),
                committed_text: if words { WORDS.into() } else { String::new() },
            },
        }
    }

    /// The toggle stop path reads `{uuid}:stopping` from the live slot. The
    /// archive link must carry the Bus uuid, or the mirror never matches the
    /// reducer's timeline and no version of the refused take is written.
    #[tokio::test]
    async fn refused_toggle_take_links_its_archive_under_the_bus_uuid() {
        let take = take(State::RecToggle, false).await;
        *take.controller.active_presentation.write().await = Some(Arc::new(take.emitter));
        let transcript = take.dir.path().join("archived.txt");
        take.controller
            .link_live_archive(Some(&format!("{TAKE}:stopping")), Some(transcript.clone()))
            .await;
        let mirror = take.controller.live_archive.lock().unwrap();
        let mirror = mirror
            .as_ref()
            .expect("refused take keeps its archive link");
        assert_eq!(mirror.session_id(), TAKE);
        assert_eq!(mirror.transcript(), transcript.as_path());
    }

    /// The ledger refused the seal, not the words: history must receive the
    /// committed document as `Raw` text instead of a `_failed` audio bag.
    #[tokio::test]
    async fn a_refused_take_with_committed_words_archives_them_as_raw_text() {
        let take = take(State::RecHold, true).await;
        assert_eq!(
            refused_take_archive(&take.refusal),
            codescribe_core::state::SessionTranscriptArchive::Committed(WORDS)
        );
    }

    /// No committed words = nothing to archive as speech; the diagnostic-only
    /// retention stays, and the reason is never persisted as transcript text.
    #[tokio::test]
    async fn a_refused_take_without_words_keeps_the_diagnostic_only_retention() {
        let take = take(State::RecHold, false).await;
        assert!(matches!(
            refused_take_archive(&take.refusal),
            codescribe_core::state::SessionTranscriptArchive::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn complete_coverage_without_seal_retains_words_without_certifying_finality() {
        let mut take = take(State::RecHold, true).await;
        let coverage = {
            let mut ledger = take.ledger.lock().unwrap();
            let speech = AcousticSpeechEvidence::measured(
                CaptureEvidenceIdentity::new(TAKE, 7),
                codescribe_core::audio::capture_receipt::CAPTURE_ENERGY_PRODUCER,
                AcousticAvailability::Observed {
                    observed_samples: 16_000,
                },
                vec![TailSampleRange {
                    session: TAKE.into(),
                    capture_epoch: 7,
                    sample_start: 0,
                    sample_end: 16_000,
                }],
            );
            let coverage = ledger.assess_seal_coverage(TAKE, 7, &speech, 0);
            assert!(coverage.status.is_complete());
            assert!(ledger.record_seal_coverage(coverage.clone()));
            take.refusal.finality = ledger.terminal_finality(TAKE, 7).into_refusal().unwrap();
            coverage
        };
        take.emitter.on_event(&EngineEvent::SealCoverage {
            receipt: coverage.clone(),
            comparison: None,
        });
        let result = take
            .controller
            .process_terminal_stop_error(
                anyhow::Error::new(take.refusal.clone()),
                |text| async move {
                    assert_eq!(text, WORDS);
                    Ok(TranscriptDelivery::SinkAccepted)
                },
            )
            .await;
        let outcome = result.as_ref().unwrap();
        assert_eq!(
            outcome.refusal.as_ref().unwrap().finality.coverage(),
            Some(&coverage)
        );
        take.controller
            .reset_finished_recording_state(&result)
            .await;
        let (terminals, _) = terminal_events(&mut take);
        assert_eq!(terminals.len(), 1);
        assert_eq!(
            terminals[0].phase,
            TranscriptProjectionPhase::CoverageRefused
        );
        assert_eq!(
            terminals[0].seal_coverage,
            Some(ProjectedSealCoverageReceipt::from(&coverage))
        );
        assert!(
            terminals[0]
                .acoustic_receipts
                .iter()
                .all(|receipt| receipt.seal_receipt.is_none())
        );
    }

    fn terminal_events(
        take: &mut Take,
    ) -> (Vec<TranscriptBusEvidenceEvent>, Vec<(String, String)>) {
        let mut terminals = Vec::new();
        let mut warnings = Vec::new();
        while let Ok(event) = take.events.try_recv() {
            match event.payload {
                IpcEventPayload::TranscriptProjection { json } => {
                    let event: TranscriptBusEvidenceEvent = serde_json::from_str(&json).unwrap();
                    if event.lifecycle_terminal {
                        terminals.push(event);
                    }
                }
                IpcEventPayload::Engine(EngineEventWire::Warning { code, message }) => {
                    warnings.push((code, message))
                }
                _ => {}
            }
        }
        (terminals, warnings)
    }

    #[tokio::test]
    async fn hold_and_toggle_refusal_preserve_receipts_and_pending_receiver() {
        for state in [State::RecHold, State::RecToggle] {
            let mut take = take(state, true).await;
            assert!(
                take.bus
                    .matches_refused_document(&take.refusal.finality, WORDS)
            );
            let controller = &take.controller;
            let result = take
                .controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript(
                                Some(TAKE),
                                &text,
                                true,
                                false,
                                CaptureTurnIntent::SingleTurn,
                                true,
                            )
                            .await
                    },
                )
                .await;
            assert!(result.as_ref().unwrap().refusal.is_some());
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(true, &result)
                .await;
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            let terminal = &terminals[0];
            assert_eq!(terminal.session_id, TAKE);
            assert_eq!(terminal.capture_epoch, 7);
            assert_eq!(terminal.rendered_text, WORDS);
            assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
            assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
            assert_eq!(
                terminal.seal_coverage,
                Some(ProjectedSealCoverageReceipt::from(
                    take.refusal.finality.coverage().unwrap()
                ))
            );
            assert!(
                terminal
                    .acoustic_receipts
                    .iter()
                    .all(|receipt| receipt.seal_receipt.is_none())
            );
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].0, "terminal_coverage_refused");
            assert!(warnings[0].1.contains("no current terminal seal"));
            assert!(!warnings[0].1.contains("coverage is incomplete"));
            assert_eq!(
                take.controller.paste_target_app_name().await.as_deref(),
                Some("original-editor")
            );
            assert_eq!(
                std::fs::read(take.refusal.audio_path.as_ref().unwrap()).unwrap(),
                b"synthetic retained WAV witness"
            );
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert_eq!(
                rows.lines()
                    .filter(|line| line.contains("\"status\":\"session_ended\""))
                    .count(),
                1
            );
            assert!(rows.contains("\"end_reason\":\"coverage_refused\""));
        }
    }

    /// Missing acoustic measurement reaches the same authenticated recovery
    /// path as measured uncovered speech: the committed words are delivered, the
    /// take WAV is retained, and the lifecycle is released.
    ///
    /// This is the guard the earlier shape would have failed. It compared the
    /// receipt against `Incomplete` specifically, so an unavailable refusal —
    /// which the ledger, recorder and emitter all now refuse on — would have
    /// been rejected here as "not the active capture" and the words lost.
    #[tokio::test]
    async fn unavailable_measurement_recovers_the_committed_words() {
        for state in [State::RecHold, State::RecToggle] {
            let mut take = take_with(state, true, false).await;
            assert_eq!(
                take.refusal.finality.coverage().unwrap().status,
                codescribe_core::pipeline::acoustic_ledger::SealCoverageStatus::Unavailable(
                    codescribe_core::pipeline::acoustic_ledger::AcousticEvidenceGap::NotObserved
                )
            );
            assert_eq!(
                take.refusal.finality.coverage().unwrap().coverage_ratio(),
                None
            );
            assert!(
                take.bus
                    .matches_refused_document(&take.refusal.finality, WORDS)
            );
            let controller = &take.controller;
            let result = take
                .controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript(
                                Some(TAKE),
                                &text,
                                true,
                                false,
                                CaptureTurnIntent::SingleTurn,
                                true,
                            )
                            .await
                    },
                )
                .await;
            let outcome = result
                .as_ref()
                .expect("unavailable measurement is a recoverable refusal");
            assert!(outcome.refusal.is_some());
            assert!(outcome.transcript_present);
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(true, &result)
                .await;
            take.controller
                .reset_finished_recording_state(&result)
                .await;

            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            let terminal = &terminals[0];
            assert_eq!(terminal.rendered_text, WORDS, "the words must survive");
            assert_eq!(terminal.phase, TranscriptProjectionPhase::CoverageRefused);
            assert_eq!(terminal.delivery, TranscriptDelivery::ComposerPending);
            let projected = terminal
                .seal_coverage
                .as_ref()
                .expect("the refusal projects its coverage evidence");
            assert_eq!(projected.status, "unavailable");
            assert_eq!(
                projected.unavailable_reason.as_deref(),
                Some("not_observed")
            );
            assert_eq!(
                projected.coverage_ratio, None,
                "the Bus must not render an unknown extent as a covered fraction"
            );
            assert_eq!(
                projected,
                &ProjectedSealCoverageReceipt::from(take.refusal.finality.coverage().unwrap())
            );
            assert!(
                terminal
                    .acoustic_receipts
                    .iter()
                    .all(|receipt| receipt.seal_receipt.is_none()),
                "no occurrence may acquire a terminal seal on this path"
            );
            assert_eq!(warnings.len(), 1);
            assert_eq!(warnings[0].0, "terminal_coverage_refused");
            assert!(warnings[0].1.contains("no current terminal seal"));
            assert!(!warnings[0].1.contains("coverage is incomplete"));
            assert_eq!(
                std::fs::read(take.refusal.audio_path.as_ref().unwrap()).unwrap(),
                b"synthetic retained WAV witness"
            );
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert!(rows.contains("\"end_reason\":\"coverage_refused\""));
            assert!(
                !rows.contains("\"coverage_ratio\""),
                "an absent ratio must be absent on the wire too: {rows}"
            );
        }
    }

    #[tokio::test]
    async fn empty_and_string_refusals_never_call_delivery_and_remain_visible() {
        for typed in [false, true] {
            let mut take = take(State::RecHold, false).await;
            let error = if typed {
                anyhow::Error::new(take.refusal.clone())
            } else {
                anyhow::anyhow!("terminal seal refused")
            };
            let result = take
                .controller
                .process_terminal_stop_error(error, |_| async {
                    panic!("no authenticated words may reach a receiver")
                })
                .await;
            assert!(result.is_err());
            take.controller
                .reset_finished_recording_state(&result)
                .await;
            take.controller
                .handle_processed_recording_result(false, &result)
                .await;
            let (terminals, warnings) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].phase, TranscriptProjectionPhase::Error);
            assert_eq!(terminals[0].delivery, TranscriptDelivery::Unattempted);
            assert_eq!(warnings[0].0, "transcription_failed");
            if typed {
                assert!(warnings[0].1.contains("without a current terminal seal"));
                assert!(!warnings[0].1.contains("incomplete speech coverage"));
            }
            let rows = std::fs::read_to_string(take.dir.path().join("bus.jsonl")).unwrap();
            assert!(rows.contains(if typed {
                "coverage_refused_empty"
            } else {
                "transcription_failed"
            }));
        }
    }

    #[tokio::test]
    async fn refused_sink_failure_is_retained_and_never_accepted() {
        let mut take = take(State::RecToggle, true).await;
        let controller = &take.controller;
        let result = take
            .controller
            .process_terminal_stop_error(
                anyhow::Error::new(take.refusal.clone()),
                |text| async move {
                    assert_eq!(text, WORDS);
                    assert_eq!(
                        controller.paste_target_app_name().await.as_deref(),
                        Some("original-editor")
                    );
                    controller
                        .finish_stop_delivery(Err(anyhow::anyhow!("receiver unavailable")), true)
                        .await
                },
            )
            .await;
        assert!(result.as_ref().unwrap_err().is::<StopDeliveryFailure>());
        take.controller
            .reset_finished_recording_state(&result)
            .await;
        take.controller
            .handle_processed_recording_result(false, &result)
            .await;
        let (terminals, warnings) = terminal_events(&mut take);
        assert_eq!(terminals[0].phase, TranscriptProjectionPhase::Error);
        assert_eq!(terminals[0].rendered_text, WORDS);
        assert_eq!(terminals[0].delivery, TranscriptDelivery::Retained);
        assert!(warnings[0].1.contains("receiver unavailable"));
        assert!(warnings[0].1.contains("no current terminal seal"));
        assert!(!warnings[0].1.contains("coverage is incomplete"));
        assert_eq!(warnings[0].0, "transcription_failed");
    }

    #[tokio::test]
    async fn forged_coverage_text_and_successor_identity_cannot_authorize_handoff() {
        let mut take = take(State::RecToggle, true).await;
        let mut forged = take.refusal.finality.coverage().unwrap().clone();
        forged.max_uncovered_samples += 1;
        take.emitter.on_event(&EngineEvent::SealCoverage {
            receipt: forged.clone(),
            comparison: None,
        });
        assert!(
            take.bus
                .matches_refused_document(&take.refusal.finality, WORDS)
        );
        assert!(
            !take
                .bus
                .matches_refused_document(&fixture_refusal(&forged), WORDS)
        );
        for mutation in 0..3 {
            let mut refusal = take.refusal.clone();
            match mutation {
                0 => refusal.committed_text = "preview is not committed".into(),
                1 => refusal.finality = fixture_refusal(&forged),
                _ => *take.controller.session_id.write().await = Some("successor-capture".into()),
            }
            let result = take
                .controller
                .process_terminal_stop_error(anyhow::Error::new(refusal), |_| async {
                    panic!("foreign or unauthenticated text reached handoff")
                })
                .await;
            assert!(result.is_err());
        }
        assert_eq!(
            take.controller.session_id.read().await.as_deref(),
            Some("successor-capture")
        );
        assert!(terminal_events(&mut take).0.is_empty());
    }

    #[tokio::test]
    async fn hold_refusal_routes_once_to_original_sink_with_exact_disposition() {
        for (delivery, expected) in [
            (
                OverlayPasteDelivery::PasteRequested,
                TranscriptDelivery::SinkAccepted,
            ),
            (
                OverlayPasteDelivery::CopiedToClipboard,
                TranscriptDelivery::CopiedToClipboard,
            ),
            (
                OverlayPasteDelivery::DeferredInsertArmed,
                TranscriptDelivery::DeferredInsertArmed,
            ),
            (OverlayPasteDelivery::Noop, TranscriptDelivery::Retained),
            (
                OverlayPasteDelivery::AccessibilityPermissionNeeded,
                TranscriptDelivery::Retained,
            ),
        ] {
            let mut take = take(State::RecHold, true).await;
            let controller = &take.controller;
            let mut config = controller.get_config().await;
            config.paste_mode = PasteMode::Safe;
            config.quick_notes_enabled = false;
            let calls = AtomicUsize::new(0);
            let call_count = &calls;
            let result = controller
                .process_terminal_stop_error(
                    anyhow::Error::new(take.refusal.clone()),
                    |text| async move {
                        controller
                            .deliver_stop_transcript_with_sink(
                                Some(TAKE),
                                &text,
                                (false, false, CaptureTurnIntent::HandsFree, true),
                                &config,
                                |route, text, target| async move {
                                    call_count.fetch_add(1, Ordering::SeqCst);
                                    assert_eq!(route, DeliveryRoute::ClipboardPaste);
                                    assert_eq!(text, WORDS);
                                    assert_eq!(target.as_deref(), Some("original-editor"));
                                    Ok(OverlayPasteResult {
                                        delivery,
                                        target_app_name: target,
                                        frontmost_app_name: Some("original-editor".into()),
                                        deferred_insert_shortcut: None,
                                        deferred_insert_failure: None,
                                    })
                                },
                            )
                            .await
                    },
                )
                .await;
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(result.is_ok(), expected != TranscriptDelivery::Retained);
            controller.reset_finished_recording_state(&result).await;
            controller
                .handle_processed_recording_result(false, &result)
                .await;
            let (terminals, _) = terminal_events(&mut take);
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0].rendered_text, WORDS);
            assert_eq!(terminals[0].delivery, expected);
        }
    }

    #[tokio::test]
    async fn refused_wav_uses_original_capture_name_without_claiming_archive_success() {
        let take = take(State::RecToggle, true).await;
        let root = take.dir.path().join("retention");
        let source = take.refusal.audio_path.as_ref().unwrap();
        let result = retain_session_audio_at(
            Some("refusal-capture:stopping"),
            source,
            codescribe_core::state::SessionTranscriptArchive::Unavailable("incomplete coverage"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |_, _| None,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("daily audio archive failed")
        );
        assert_eq!(
            std::fs::read(source).unwrap(),
            b"synthetic retained WAV witness"
        );
        assert_eq!(
            std::fs::read(root.join("sessions/refusal-capture.wav")).unwrap(),
            b"synthetic retained WAV witness"
        );
        assert!(!root.join("sessions/refusal-capture:stopping.wav").exists());
        assert!(
            take.bus
                .matches_refused_document(&take.refusal.finality, WORDS)
        );
    }

    #[tokio::test]
    async fn delivery_claim_rejects_repeat_and_admits_a_successor() {
        let controller = RecordingController::new_without_keychain();
        for (id, allowed) in [
            (TAKE, true),
            ("refusal-capture:stopping", false),
            ("successor-capture", true),
        ] {
            let result = controller
                .deliver_stop_transcript_with_sink(
                    Some(id),
                    WORDS,
                    (true, false, CaptureTurnIntent::SingleTurn, true),
                    &controller.get_config().await,
                    |_, _, _| async { panic!("composer must not paste") },
                )
                .await;
            assert_eq!(result.is_ok(), allowed);
            assert_eq!(
                *controller.delivery_disposition.read().await,
                TranscriptDelivery::ComposerPending
            );
        }
    }

    #[tokio::test]
    async fn light_plus_controller_one_document_paint_per_revision() {
        let count = 5_u64;
        let session = "overlay-one-document";
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session.into(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                dir.path().join("synthetic.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let buffer = Arc::new(Mutex::new(String::new()));
        let (sender, mut receiver) = broadcast::channel(256);
        let pipeline = RecordingController::build_recording_event_sink(
            Arc::clone(&buffer),
            RecordingEventSinkOptions {
                preview_deltas_enabled: false,
                sentence_pause_sec: 0.7,
                seal_ledger_enabled: true,
                apple_live_enabled: true,
                speech_observer: None,
            },
            sender,
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Arc::default(),
        );
        pipeline.event_sink.on_capture_opened(session, 7);
        pipeline.presentation.set_literal_delivery(true);
        while receiver.try_recv().is_ok() {}
        let mut owners = Vec::new();
        for i in 0..count {
            let owner = OccurrenceIdentity::new(session, 7, i * 16_000, (i + 1) * 16_000);
            let calibration = EnergyCalibration {
                version: "synthetic-qualification".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            };
            let evidence = AcousticEvidence {
                occurrence: owner.clone(),
                duration_ms: 1000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(owner.sample_start),
                vad_close_sample: Some(owner.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            let event = {
                let mut ledger = ledger.lock().unwrap();
                assert!(ledger.qualify(&evidence, &calibration).is_qualified());
                let observation =
                    ObservationIdentity::new(ObservationProducer::Apple, i + 1, 0, owner.clone());
                let receipt = ledger.admit(&observation, "Iwo");
                EngineEvent::LedgerMutation {
                    observation,
                    label: "Iwo".into(),
                    receipt,
                }
            };
            pipeline.event_sink.on_event(&event);
            owners.push(owner);
        }
        assert!(pipeline.presentation.wait_paint_published().await);
        let expected = vec!["Iwo"; count as usize].join(" ");
        assert_eq!(*buffer.lock().await, expected);
        {
            let ledger = ledger.lock().unwrap();
            assert_eq!(ledger.len(), count as usize);
            for owner in &owners {
                assert_eq!(ledger.text_of(owner), Some("Iwo"));
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }
        let mut rows = std::collections::BTreeMap::<u64, Vec<TranscriptBusEvidenceEvent>>::new();
        while let Ok(event) = receiver.try_recv() {
            if let IpcEventPayload::TranscriptProjection { json } = event.payload {
                let row: TranscriptBusEvidenceEvent = serde_json::from_str(&json).unwrap();
                assert!(!row.lifecycle_terminal);
                rows.entry(row.reducer_revision).or_default().push(row);
            }
        }
        assert_eq!(rows.len(), count as usize);
        let multiplicities = rows.values().map(Vec::len).collect::<Vec<_>>();
        for (index, group) in rows.values().enumerate() {
            assert!(
                group
                    .iter()
                    .all(|row| row.rendered_text == group[0].rendered_text)
            );
            let receipts = group
                .iter()
                .flat_map(|row| row.acoustic_receipts.iter())
                .collect::<Vec<_>>();
            assert_eq!(
                receipts.len(),
                index + 1,
                "every authentic owner survives, exactly once per revision"
            );
            for (i, receipt) in receipts.iter().enumerate() {
                assert_eq!(receipt.session_id, session);
                assert_eq!(receipt.capture_epoch, 7);
                assert_eq!(receipt.sample_start, i as u64 * 16_000);
                assert_eq!(receipt.sample_end, (i as u64 + 1) * 16_000);
                assert!(!receipt.acoustic_serial.is_empty());
                assert!(!receipt.layer_decision_receipts.is_empty());
            }
        }
        assert_eq!(rows.values().last().unwrap()[0].rendered_text, expected);
        eprintln!(
            "LIGHT_PLUS_IPC_JSON {}",
            serde_json::json!({"owners":count,"full_document_projections_per_revision":multiplicities,"actual_capture_or_gui":false})
        );
        assert!(
            multiplicities.iter().all(|n| *n == 1),
            "one overlay document revision must not be resent once per occurrence; occurrence evidence must remain available without repeating the full paint"
        );
    }

    #[tokio::test]
    async fn light_plus_equal_raw_revision_keeps_terminal_delivery() {
        let count = 5_u64;
        let session = "overlay-terminal-delivery";
        let dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: session.into(),
                    mode: TranscriptMode::Dictation,
                    has_latched_target: true,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                dir.path().join("synthetic.jsonl"),
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        let ledger = Arc::new(std::sync::Mutex::new(AcousticLedger::new()));
        let buffer = Arc::new(Mutex::new(String::new()));
        let (sender, mut receiver) = broadcast::channel(256);
        let pipeline = RecordingController::build_recording_event_sink(
            Arc::clone(&buffer),
            RecordingEventSinkOptions {
                preview_deltas_enabled: false,
                sentence_pause_sec: 0.7,
                seal_ledger_enabled: true,
                apple_live_enabled: true,
                speech_observer: None,
            },
            sender.clone(),
            Some(Arc::clone(&bus)),
            Some(Arc::clone(&ledger)),
            Arc::default(),
        );
        pipeline.event_sink.on_capture_opened(session, 7);
        pipeline.presentation.set_literal_delivery(true);
        while receiver.try_recv().is_ok() {}
        let mut owners = Vec::new();
        for i in 0..count {
            let owner = OccurrenceIdentity::new(session, 7, i * 16_000, (i + 1) * 16_000);
            let calibration = EnergyCalibration {
                version: "synthetic-qualification".into(),
                min_energy_integral: 1.0,
                min_valley_samples: 1,
            };
            let evidence = AcousticEvidence {
                occurrence: owner.clone(),
                duration_ms: 1000.0,
                energy_integral: 10.0,
                mean_rms_dbfs: -12.0,
                peak_dbfs: -3.0,
                vad_open_sample: Some(owner.sample_start),
                vad_close_sample: Some(owner.sample_end),
                evidence_calibration_version: calibration.version.clone(),
            };
            let event = {
                let mut ledger = ledger.lock().unwrap();
                assert!(ledger.qualify(&evidence, &calibration).is_qualified());
                let observation =
                    ObservationIdentity::new(ObservationProducer::Apple, i + 1, 0, owner.clone());
                let receipt = ledger.admit(&observation, "Iwo");
                EngineEvent::LedgerMutation {
                    observation,
                    label: "Iwo".into(),
                    receipt,
                }
            };
            pipeline.event_sink.on_event(&event);
            owners.push(owner);
        }
        assert!(pipeline.presentation.wait_paint_published().await);
        let expected = vec!["Iwo"; count as usize].join(" ");
        assert_eq!(*buffer.lock().await, expected);
        {
            let ledger = ledger.lock().unwrap();
            assert_eq!(ledger.len(), count as usize);
            for owner in &owners {
                assert_eq!(ledger.text_of(owner), Some("Iwo"));
            }
            assert!(ledger.word_deletions().is_empty());
            assert_eq!(ledger.conservation().residue(), 0);
        }

        let mut last = None;
        while let Ok(event) = receiver.try_recv() {
            if let IpcEventPayload::TranscriptProjection { json } = event.payload {
                let row: TranscriptBusEvidenceEvent = serde_json::from_str(&json).unwrap();
                assert!(!row.lifecycle_terminal);
                last = Some(row);
            }
        }
        let previous = last.expect("real document projection");
        let slot = RwLock::new(Some(Arc::clone(&bus)));
        RecordingController::end_transcript_bus(
            &slot,
            TranscriptSessionEndReason::Completed,
            true,
            TranscriptDelivery::SinkAccepted,
            Some("selected delivery text".into()),
            Some(&sender),
        )
        .await;
        let mut ended = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let IpcEventPayload::TranscriptProjection { json } = event.payload {
                ended.push(serde_json::from_str::<TranscriptBusEvidenceEvent>(&json).unwrap());
            }
        }
        assert_eq!(
            ended.len(),
            1,
            "terminal lifecycle must arrive exactly once even without a new Raw revision"
        );
        let ended = &ended[0];
        assert_eq!(ended.reducer_revision, previous.reducer_revision);
        assert!(ended.sequence > previous.sequence);
        assert!(ended.lifecycle_terminal);
        assert!(ended.terminal);
        assert_eq!(ended.delivery, TranscriptDelivery::SinkAccepted);
        assert_eq!(ended.rendered_text, expected);
        assert_eq!(
            ended.delivery_text.as_deref(),
            Some("selected delivery text")
        );
        assert!(ended.can_copy && ended.can_retranscribe);
        assert!(slot.read().await.is_none());
        RecordingController::end_transcript_bus(
            &slot,
            TranscriptSessionEndReason::Completed,
            true,
            TranscriptDelivery::SinkAccepted,
            None,
            Some(&sender),
        )
        .await;
        assert!(
            receiver.try_recv().is_err(),
            "repeated closure cannot issue a second delivery"
        );
    }
}

/// W2 contracts: UNRUN until the integrator closes the joined structure.
/// Real controller/empty recorder/temporary Bus, no provider or microphone.
#[cfg(test)]
#[serial_test::serial]
mod owned_capture_settlement_tests {
    use super::*;
    use crate::presentation::transcript_bus::CleanTranscriptEvent;

    async fn fixture() -> (
        Arc<RecordingController>,
        mpsc::UnboundedReceiver<CaptureSettlementStage>,
        tempfile::TempDir,
    ) {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::RecToggle).await;
        *controller.assistive_mode.write().await = true;
        *controller.session_id.write().await = Some("owned".to_string());
        controller
            .recorder
            .lock()
            .await
            .as_mut()
            .expect("empty recorder fixture")
            .set_capture_turn_intent(CaptureTurnIntent::SingleTurn);
        let dir = tempfile::tempdir().unwrap();
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "owned".to_string(),
                mode: TranscriptMode::Agent,
                has_latched_target: false,
                latched_target_is_self: false,
                audience: None,
                badge_only: false,
            },
            dir.path().join("events.jsonl"),
            None,
        )
        .unwrap();
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::new(bus));
        let (sender, stages) = mpsc::unbounded_channel();
        *controller.settlement_observer.lock().unwrap() = Some(sender);
        (controller, stages, dir)
    }

    async fn stage(
        stages: &mut mpsc::UnboundedReceiver<CaptureSettlementStage>,
        expected: CaptureSettlementStage,
    ) {
        loop {
            let observed = tokio::time::timeout(STOP_TIMEOUT * 2, stages.recv())
                .await
                .expect("owner must acknowledge arrival")
                .expect("observer remains installed");
            if observed == expected {
                return;
            }
        }
    }

    fn rows(dir: &tempfile::TempDir) -> Vec<CleanTranscriptEvent> {
        std::fs::read_to_string(dir.path().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn every_stop_route_joins_one_operation_past_caller_budget() {
        for route in 0..5 {
            let (controller, mut stages, dir) = fixture().await;
            if route == 1 || route == 2 {
                controller.set_state(State::RecHold).await;
            }
            let held = controller.hold_mode.write().await;
            let caller = tokio::spawn({
                let controller = Arc::clone(&controller);
                async move {
                    match route {
                        0 => controller.stop_recording_from_external_surface().await,
                        1 => {
                            controller
                                .handle_hotkey_event(HotkeyInput {
                                    key_type: HotkeyType::Hold,
                                    action: HotkeyAction::Up,
                                    assistive: true,
                                    hold_mode: HoldMode::Chat,
                                    force_raw: false,
                                    force_ai: false,
                                })
                                .await
                        }
                        2 | 3 => {
                            controller
                                .handle_hotkey_event(HotkeyInput {
                                    key_type: HotkeyType::Toggle,
                                    action: HotkeyAction::Press,
                                    assistive: false,
                                    hold_mode: HoldMode::Raw,
                                    force_raw: true,
                                    force_ai: false,
                                })
                                .await
                        }
                        _ => controller.finish_recording().await,
                    }
                }
            });
            stage(&mut stages, CaptureSettlementStage::Admitted).await;
            let task_id = controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .id();
            assert!(
                caller
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("Pending")
            );
            assert_eq!(controller.current_state().await, State::Busy);
            if route == 1 || route == 2 {
                assert!(
                    controller.serial_lock.try_lock().is_err(),
                    "hold intent is still being frozen before capture close"
                );
            } else {
                assert!(
                    controller.serial_lock.try_lock().is_ok(),
                    "closed capture must release gesture admission while reset waits"
                );
            }
            assert_eq!(rows(&dir).len(), 1);
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                CaptureStopOutcome::Pending
            );
            assert_eq!(
                controller
                    .capture_settlement
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .task
                    .id(),
                task_id
            );
            drop(held);
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                CaptureStopOutcome::Stopped
            );
            assert_eq!(rows(&dir).len(), 2, "route {route}: exactly one terminal");
            assert_eq!(controller.current_state().await, State::Idle);
        }
    }

    #[tokio::test]
    async fn stop_during_published_start_is_retained_for_every_stop_gesture() {
        for route in 0..4 {
            let (controller, _, dir) = fixture().await;
            if route == 1 || route == 2 {
                controller.set_state(State::RecHold).await;
            }
            // Actual start publishes RecHold/RecToggle before releasing this lock.
            // No microphone or model is opened by this empty-recorder fixture.
            let held = controller.serial_lock.lock().await;
            let mut call = Box::pin(async {
                match route {
                    0 => controller.stop_recording_from_external_surface().await,
                    _ => {
                        controller
                            .handle_hotkey_event(HotkeyInput {
                                key_type: if route == 1 {
                                    HotkeyType::Hold
                                } else {
                                    HotkeyType::Toggle
                                },
                                action: if route == 1 {
                                    HotkeyAction::Up
                                } else {
                                    HotkeyAction::Press
                                },
                                assistive: route == 1,
                                hold_mode: if route == 1 {
                                    HoldMode::Chat
                                } else {
                                    HoldMode::Raw
                                },
                                force_raw: route == 2,
                                force_ai: false,
                            })
                            .await
                    }
                }
            });
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(
                std::future::Future::poll(call.as_mut(), &mut context).is_pending(),
                "route {route}: Stop must be retained while its published start finishes"
            );
            assert_eq!(
                controller
                    .capture_settlement
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .capture_id,
                "owned"
            );
            assert_eq!(rows(&dir).len(), 1, "Stop has not settled yet");
            drop(held);
            call.await
                .expect("the original Stop settles after admission releases");
            assert_eq!(controller.current_state().await, State::Idle);
            assert!(controller.session_id.read().await.is_none());
            let events = rows(&dir);
            assert_eq!(events.len(), 2, "route {route}: no spontaneous successor");
            assert_eq!(events[0].session_id, events[1].session_id);
            assert_eq!(events[1].status, "session_ended");
        }
    }

    #[tokio::test]
    async fn stop_current_waits_for_its_snapshot_and_cannot_target_successor() {
        let (controller, _, dir) = fixture().await;
        let held = controller.serial_lock.lock().await;
        let mut call = Box::pin(controller.stop_current_capture());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(call.as_mut(), &mut context).is_pending());
        assert_eq!(
            controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .capture_id,
            "owned"
        );
        *controller.session_id.write().await = Some("replacement".into());
        drop(held);
        assert_eq!(call.await.unwrap(), CaptureStopOutcome::ForeignCapture);
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("replacement")
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(rows(&dir).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_current_timeout_during_start_keeps_the_terminal_owner() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.serial_lock.lock().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_current_capture().await }
        });
        stage(&mut stages, CaptureSettlementStage::Registered).await;
        assert_eq!(caller.await.unwrap().unwrap(), CaptureStopOutcome::Pending);
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(rows(&dir).len(), 2);
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_external_caller_keeps_terminal_owner() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_recording_from_external_surface().await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_pending_preserves_owner_and_closes_successor_admission() {
        let (controller, mut stages, dir) = fixture().await;
        controller.request_capture_shutdown();
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_current_capture().await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        assert_eq!(caller.await.unwrap().unwrap(), CaptureStopOutcome::Pending);
        assert!(!controller.capture_shutdown_settled());
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        // Observe the actual task exit, not merely its watch publication.
        let operation = controller
            .capture_settlement
            .lock()
            .unwrap()
            .take()
            .unwrap();
        operation.task.await.unwrap();
        assert!(controller.capture_shutdown_settled());
        assert!(controller.start_composer_turn_recording().await.is_err());
        let generation = controller.hold_start_generation.load(Ordering::SeqCst);
        assert!(
            controller
                .schedule_hold_start(false, generation)
                .await
                .is_err()
        );
        assert!(controller.start_conversation_mode().await.is_err());
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test]
    async fn external_failure_and_named_retry_share_one_failed_terminal() {
        let (controller, _, dir) = fixture().await;
        *controller.recorder.lock().await = None;
        let failure = controller
            .stop_recording_from_external_surface()
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("recorder unavailable"));
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        let events = rows(&dir);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }

    #[tokio::test]
    async fn hotkey_stop_decision_retains_the_state_used_for_routing() {
        let controller = RecordingController::new_without_keychain();
        let event = HotkeyInput {
            key_type: HotkeyType::Toggle,
            action: HotkeyAction::Press,
            assistive: false,
            hold_mode: HoldMode::Raw,
            force_raw: true,
            force_ai: false,
        };
        for (initial, expected_stop) in [
            (State::Idle, false),
            (State::RecHold, true),
            (State::RecToggle, true),
            (State::Busy, true),
        ] {
            controller.set_state(initial).await;
            let (state, stop) = controller.hotkey_stop_decision(&event).unwrap();
            controller.set_state(State::Idle).await;
            assert_eq!(state, initial);
            assert_eq!(stop, expected_stop);
            assert_eq!(
                event.decision_line(Some(state), stop),
                event.decision_line(Some(initial), expected_stop)
            );
        }
        let _held = controller.state.write().await;
        assert!(controller.hotkey_stop_decision(&event).is_err());
    }

    #[tokio::test]
    async fn idle_stop_invalidates_only_unadmitted_hold_generation() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        let generation = controller.hold_start_generation.load(Ordering::SeqCst);
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::NoLiveCapture
        );
        assert_ne!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            generation
        );
        let generation = controller.hold_start_generation.load(Ordering::SeqCst);
        assert_eq!(
            controller.stop_capture_if_owned("foreign").await.unwrap(),
            CaptureStopOutcome::NoLiveCapture
        );
        assert_eq!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            generation
        );
    }

    /// A stop that never closes a capture drops its receipt sender without a
    /// receipt. The gesture then settles on the result; a lost receipt is
    /// never an outcome of its own.
    #[tokio::test]
    async fn admitted_stop_settles_on_its_result_after_the_receipt_sender_is_dropped() {
        for (settled, ok) in [
            (Ok(CaptureStopOutcome::NoLiveCapture), true),
            (Ok(CaptureStopOutcome::ForeignCapture), false),
            (
                Err("Toggle-adjudicate: recorder unavailable".to_string()),
                false,
            ),
        ] {
            let (result_tx, result_rx) = watch::channel(None);
            let (closed_tx, closed_rx) = watch::channel(false);
            drop(closed_tx);
            let waiter = tokio::spawn(RecordingController::await_admitted_stop(
                result_rx, closed_rx,
            ));
            tokio::task::yield_now().await;
            assert!(!waiter.is_finished(), "a lost receipt is not an outcome");
            result_tx.send_replace(Some(settled));
            let outcome = waiter.await.unwrap();
            assert_eq!(outcome.is_ok(), ok);
            if let Err(error) = outcome {
                let message = format!("{error:#}");
                assert!(!message.contains("receipt"), "{message}");
                assert!(
                    message.contains("ForeignCapture") || message.contains("recorder unavailable"),
                    "{message}"
                );
            }
        }
    }

    #[tokio::test]
    async fn busy_without_a_stop_receipt_is_not_shutdown_success() {
        let controller = Arc::new(RecordingController::new_without_keychain());
        controller.set_state(State::Busy).await;
        controller.request_capture_shutdown();
        assert_eq!(
            controller.stop_current_capture().await.unwrap(),
            CaptureStopOutcome::AdmissionUnavailable
        );
        assert!(!controller.capture_shutdown_settled());
        assert_eq!(controller.current_state().await, State::Busy);
    }

    #[tokio::test(start_paused = true)]
    async fn native_stop_acknowledges_close_while_terminal_reset_remains_owned() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move {
                controller
                    .admit_hotkey_event(HotkeyInput {
                        key_type: HotkeyType::Toggle,
                        action: HotkeyAction::Press,
                        assistive: false,
                        hold_mode: HoldMode::Raw,
                        force_raw: false,
                        force_ai: true,
                    })
                    .await
            }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        tokio::time::timeout(Duration::from_secs(1), caller)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(controller.stopped_capture_feed_closed());
        assert!(controller.serial_lock.try_lock().is_ok());
        assert!(controller.recorder.try_lock().is_ok());
        assert_eq!(controller.current_state().await, State::Busy);
        assert_eq!(
            rows(&dir).len(),
            1,
            "close admission is not a terminal receipt"
        );
        assert!(
            !controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .is_finished()
        );
        assert_eq!(
            controller
                .start_toggle_recording(false, CaptureTurnIntent::SingleTurn)
                .await
                .unwrap(),
            CaptureAdmission::NotAdmitted
        );
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn held_hold_mode_cannot_block_public_stop_and_settles_once_after_release() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        // The obstacle is STILL HELD during the independent outer deadline.
        let outcome = tokio::time::timeout(STOP_TIMEOUT * 2, caller)
            .await
            .expect("public Stop must return while hold_mode stays locked")
            .unwrap()
            .unwrap();
        assert_eq!(outcome, CaptureStopOutcome::Pending);
        assert_eq!(controller.current_state().await, State::Busy);
        assert!(controller.serial_lock.try_lock().is_ok());
        assert!(controller.recorder.try_lock().is_ok());
        assert_eq!(rows(&dir).len(), 1, "pending is not a terminal receipt");
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(controller.current_state().await, State::Idle);
        let events = rows(&dir);
        assert_eq!(events.len(), 2, "one start and exactly one terminal");
        assert_eq!(
            events[1].end_reason,
            Some(TranscriptSessionEndReason::Completed)
        );
        assert!(controller.active_transcript_bus.read().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_caller_and_duplicate_keep_one_owner_and_cannot_reset_successor() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.hold_mode.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Resetting).await;
        let task_id = controller
            .capture_settlement
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .task
            .id();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(
            controller
                .capture_settlement
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .task
                .id(),
            task_id
        );
        // Use the production start admission, which checks Busy under the
        // serial lock. Owning that lock alone never authorizes a successor.
        assert_eq!(
            controller
                .start_toggle_recording(false, CaptureTurnIntent::SingleTurn)
                .await
                .unwrap(),
            CaptureAdmission::NotAdmitted
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("owned:stopping")
        );
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        // After the predecessor's terminal task exits, its completed receipt
        // cannot reset a later admitted capture.
        *controller.session_id.write().await = Some("successor".into());
        controller.set_state(State::RecToggle).await;
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("successor")
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(rows(&dir).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn pre_admission_wait_is_pending_and_rechecks_foreign_identity_after_release() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.serial_lock.lock().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Registered).await;
        let pending = tokio::time::timeout(STOP_TIMEOUT * 2, caller)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(pending, CaptureStopOutcome::Pending);
        assert_eq!(controller.current_state().await, State::RecToggle);
        *controller.session_id.write().await = Some("foreign".to_string());
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::ForeignCapture
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("foreign")
        );
        assert_eq!(rows(&dir).len(), 1, "foreign take and Bus untouched");
    }

    #[tokio::test(start_paused = true)]
    async fn routing_lock_is_inside_public_budget_too() {
        let (controller, mut stages, dir) = fixture().await;
        let held = controller.state.write().await;
        let caller = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.stop_capture_if_owned("owned").await }
        });
        stage(&mut stages, CaptureSettlementStage::Registered).await;
        assert_eq!(
            tokio::time::timeout(STOP_TIMEOUT * 2, caller)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            CaptureStopOutcome::Pending
        );
        assert_eq!(rows(&dir).len(), 1);
        drop(held);
        assert_eq!(
            controller.stop_capture_if_owned("owned").await.unwrap(),
            CaptureStopOutcome::Stopped
        );
    }

    #[tokio::test]
    async fn unavailable_registration_admits_no_task_or_mutation() {
        let (controller, _, dir) = fixture().await;
        let outcome = {
            let held = controller.capture_settlement.lock().unwrap();
            let mut call = Box::pin(controller.stop_capture_if_owned("owned"));
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            let std::task::Poll::Ready(result) =
                std::future::Future::poll(call.as_mut(), &mut context)
            else {
                panic!("registration contention must not suspend");
            };
            assert!(held.is_none());
            result.unwrap()
        };
        assert_eq!(outcome, CaptureStopOutcome::AdmissionUnavailable);
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(rows(&dir).len(), 1);
    }

    #[tokio::test]
    async fn absent_and_already_stopping_are_not_success() {
        for (identity, state, expected) in [
            (None, State::Idle, CaptureStopOutcome::NoLiveCapture),
            (
                Some("owned:stopping"),
                State::Busy,
                CaptureStopOutcome::AlreadyStopping,
            ),
        ] {
            let controller = Arc::new(RecordingController::new_without_keychain());
            *controller.session_id.write().await = identity.map(str::to_string);
            controller.set_state(state).await;
            assert_eq!(
                controller.stop_capture_if_owned("owned").await.unwrap(),
                expected
            );
            assert_eq!(controller.current_state().await, state);
        }
    }

    #[tokio::test]
    async fn explicit_recorder_failure_is_retained_as_failure_with_one_terminal() {
        let (controller, _, dir) = fixture().await;
        *controller.recorder.lock().await = None;
        let failure = controller
            .stop_capture_if_owned("owned")
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("recorder unavailable"));
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        assert_eq!(controller.current_state().await, State::Idle);
        assert_eq!(rows(&dir).len(), 2);
        assert_eq!(
            rows(&dir)[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        *controller.session_id.write().await = Some("successor".into());
        controller.set_state(State::RecToggle).await;
        assert_eq!(
            controller
                .stop_capture_if_owned("owned")
                .await
                .unwrap_err()
                .to_string(),
            failure
        );
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("successor")
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(
            rows(&dir).len(),
            2,
            "duplicate failure cannot end the successor"
        );
    }
}

#[cfg(test)]
mod serving_status_producer_falsifiers {
    use super::*;

    /// Settings "Active STT" reads `serving_status::current_last_serving()`.
    /// The producer left with the old adjudicating stop path (`ac6d399b3`);
    /// since then the row was always "Not yet served". A finished toggle stop
    /// must publish the engine the live session actually ran on — the
    /// recorder's label, never the configured `stt_engine`.
    ///
    /// `StreamingRecorder::new` opens no device, and `stop()` on a session
    /// that never started returns an empty transcript, so this drives the real
    /// `stop_toggle_and_adjudicate_inner` success branch without CoreAudio.
    ///
    /// The preemption lane keeps a foreign test's take start from preempting
    /// this stop through the process-global stop-paste slot: a preempted stop
    /// legitimately skips the verdict, which is the race this test would then
    /// misreport as a missing publish.
    #[tokio::test]
    async fn toggle_stop_publishes_the_live_session_engine() {
        let _preemption_lane = crate::test_env::StopPastePreemptionTestLane::acquire();
        let _serialized = serving_status::test_store_lock().lock().await;
        serving_status::clear_last_serving();
        let controller = RecordingController::new_without_keychain();
        assert!(
            controller.recorder.lock().await.is_some(),
            "test controller must own a StreamingRecorder (no device needed)"
        );
        controller.set_state(State::RecToggle).await;

        let serial = Arc::clone(&controller.serial_lock).lock_owned().await;
        controller
            .stop_toggle_and_adjudicate_inner(None, serial, None)
            .await
            .expect("idle recorder stops cleanly");

        let verdict = serving_status::current_last_serving()
            .expect("stop path publishes the serving verdict");
        assert_eq!(verdict.engine, "local_apple");
        assert_eq!(verdict.routing_mode, serving_status::LIVE_ROUTING_MODE);
        assert_eq!(verdict.disposition, None);
        assert!(!verdict.fallback_used);
        assert_eq!(controller.current_state().await, State::Idle);
        serving_status::clear_last_serving();
    }
}

#[cfg(test)]
mod admission_presentation_status_falsifiers {
    use super::*;

    /// W3-T11 baseline witness: a deterministic refusal must leave the engine
    /// warning side-channel and travel as a typed presentation projection.
    /// The refusal settles the process-wide tray to Idle, so this runs
    /// serially with the tests that assert that projection.
    #[test]
    #[serial_test::serial]
    fn acoustic_admission_refusal_uses_presentation_projection_event() {
        let (events, mut receiver) = broadcast::channel(2);
        let blocker = admission::AdmissionBlocker::CalibrationUnusable {
            device_name: "Built-in Microphone".to_string(),
            reason: "capture generation changed: measured 88200Hz/1ch, current 48000Hz/1ch"
                .to_string(),
        };

        RecordingController::broadcast_admission_refusal(
            &events,
            Some("session-1".to_string()),
            &blocker,
        );

        let event = receiver.try_recv().expect("one refusal projection");
        let payload = serde_json::to_value(event.payload).expect("serialize IPC payload");
        assert_eq!(payload["event"], "presentation_status");
        let projection: serde_json::Value = serde_json::from_str(
            payload["json"]
                .as_str()
                .expect("presentation status owns typed JSON"),
        )
        .expect("valid presentation status projection");
        assert_eq!(projection["kind"], "admission_refused");
        assert_eq!(projection["code"], "admission_calibration_unusable");
        assert_eq!(projection["session_id"], "session-1");
        assert!(
            projection["message"]
                .as_str()
                .expect("human-readable refusal")
                .contains("Settings › Audio")
        );
    }
}

#[cfg(test)]
mod c15d_settings_one_path_falsifiers {
    use super::*;
    use crate::config::Config;
    use codescribe_core::audio::capture_receipt::{
        CAPTURE_LEVEL_RECEIPT_CODE, CaptureLevelReceipt, CapturePathMeta,
    };
    use codescribe_core::config::energy_calibration::{
        EnergyCalibrationArtifact, EnergyCalibrationProfile, SOURCE_SYNTHETIC_FIXTURE,
        energy_calibration_path,
    };
    use codescribe_core::pipeline::streaming::SealLaneProbe;
    use std::ffi::OsString;

    struct DataDirGuard(Option<OsString>);

    impl DataDirGuard {
        fn install(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("CODESCRIBE_DATA_DIR");
            // SAFETY: the test is serial and restores the process variable.
            unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", path) };
            Self(previous)
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            // SAFETY: paired restoration for the serial test above.
            unsafe {
                match self.0.take() {
                    Some(previous) => std::env::set_var("CODESCRIBE_DATA_DIR", previous),
                    None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                }
            }
        }
    }

    fn calibration_receipt(device_name: &str) -> CaptureLevelReceipt {
        CaptureLevelReceipt {
            code: CAPTURE_LEVEL_RECEIPT_CODE,
            device_name: device_name.to_string(),
            sample_rate: 48_000,
            channels: 1,
            sample_count: 480_000,
            digital_zero_samples: 0,
            active_speech_samples: 240_000,
            clipping_samples: 0,
            dropout_blocks: 0,
            all_audio_median_db: -40.0,
            active_speech_median_db: -30.0,
            peak_db: -6.0,
            noise_floor_db: -80.0,
            snr_db: Some(50.0),
            threshold_db: -52.0,
            low: false,
        }
    }

    /// C15D-A structural falsifier: the controller source has one mutable
    /// settings handle, one write site and no compatibility setter.
    #[test]
    fn controller_has_exactly_one_mutable_settings_generation() {
        let source = include_str!("mod.rs");
        let retired_config_field = ["config: Arc<RwLock<", "Config>>"].concat();
        let runtime_arc_field =
            ["runtime_settings: RwLock<Arc<", "RuntimeSettingsSnapshot>>"].concat();
        let runtime_arc_write = ["*self.runtime_settings", ".write().await"].concat();
        let retired_setter = ["pub async ", "fn set_", "config"].concat();

        assert!(!source.contains(&retired_config_field));
        assert_eq!(source.matches(&runtime_arc_field).count(), 1);
        assert_eq!(source.matches(&runtime_arc_write).count(), 1);
        assert!(!source.contains(&retired_setter));
    }

    /// C15D-A projection falsifier: public config is a value projected from
    /// the current Arc, not separately stored controller state.
    #[tokio::test]
    async fn get_config_projects_current_runtime_snapshot_values() {
        let controller = RecordingController::new_without_keychain();
        let snapshot = controller.runtime_settings_arc().await;
        let projected = controller.get_config().await;

        assert_eq!(
            projected.hold_start_delay_ms,
            snapshot.values().hold_start_delay_ms
        );
        assert_eq!(projected.beep_on_start, snapshot.values().beep_on_start);
        assert_eq!(
            projected.transcription_overlay_enabled,
            snapshot.values().transcription_overlay_enabled
        );
    }

    /// C15D-A generation falsifier: idle refresh performs one Arc replacement
    /// and cannot mutate a previously selected Arc.
    #[tokio::test]
    #[serial_test::serial]
    async fn idle_refresh_replaces_one_arc_and_preserves_previous_generation() {
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let before_digest = before.digest().as_str().to_string();
        let before_delay = before.values().hold_start_delay_ms;
        let mut settings = UserSettings::load();
        settings.paste_mode = Some(PasteMode::Off);
        settings.save().unwrap();
        assert!(
            controller
                .refresh_runtime_settings_from_disk()
                .await
                .expect("refresh idle settings")
        );
        let after = controller.runtime_settings_arc().await;
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.values().paste_mode, PasteMode::Off);
        assert!(
            !controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );
        assert_eq!(
            controller.get_config().await.hold_start_delay_ms,
            after.values().hold_start_delay_ms
        );
        assert_eq!(before.digest().as_str(), before_digest.as_str());
        assert_eq!(before.values().hold_start_delay_ms, before_delay);
    }

    /// Startup-seal falsifier: launch creates the controller before Swift
    /// restores provider access, so a Keychain-only live key is absent from the
    /// first sealed generation and Layer 1 reports `live_key_missing`. The
    /// refresh that credential acquisition triggers must publish that secret to
    /// the next take without any settings write.
    #[tokio::test]
    #[serial_test::serial]
    async fn credential_acquisition_after_seal_arms_the_live_key_on_refresh() {
        use codescribe_core::config::keychain::test_support::install_bundle;
        use codescribe_core::test_isolation::EnvGuard;

        let temp = tempfile::tempdir().unwrap();
        let _data_dir = DataDirGuard::install(temp.path());
        let _live_env = EnvGuard::remove("STT_LIVE_API_KEY");
        let _retired_env = EnvGuard::remove("STT_API_KEY");
        let _empty_bundle = install_bundle(&[]);

        let controller = RecordingController::new_without_keychain();
        let sealed_at_launch = controller.runtime_settings_arc().await;
        assert!(sealed_at_launch.values().stt_live_api_key.is_none());

        let _acquired = install_bundle(&[("STT_LIVE_API_KEY", "synthetic-live")]);
        assert!(
            controller
                .refresh_runtime_settings_from_disk()
                .await
                .expect("idle refresh after credential acquisition")
        );
        let resealed = controller.get_config().await;
        assert_eq!(resealed.stt_live_api_key.as_deref(), Some("synthetic-live"));
    }

    /// W3-T11 race falsifier: after the profile is stored, the calibration
    /// path must replace the live controller Arc before Swift can issue its
    /// immediate readiness probe.
    #[tokio::test]
    #[serial_test::serial]
    async fn calibration_refresh_makes_the_new_profile_visible_before_return() {
        let temp = tempfile::tempdir().expect("temporary data dir");
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        assert_eq!(admission::calibration_status_view(&before).code, "missing");

        let measured_at = 10_000;
        let profile = EnergyCalibrationProfile::derive(
            &calibration_receipt("Fixture Mic"),
            measured_at,
            SOURCE_SYNTHETIC_FIXTURE,
        )
        .expect("fixture calibration derives");
        EnergyCalibrationArtifact::record_profile(&energy_calibration_path(), profile, measured_at)
            .expect("profile persists");

        let _serial_guard = controller.serial_lock.lock().await;
        controller
            .refresh_runtime_settings_after_calibration_locked()
            .await
            .expect("calibration refresh seals a new generation");
        let after = controller.runtime_settings_arc().await;

        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(admission::calibration_status_view(&after).code, "sealed");
        let grant = admission::evaluate_admission_readiness_at(
            &after,
            Ok(CapturePathMeta {
                device_name: "Fixture Mic".to_string(),
                sample_rate: 48_000,
                channels: 1,
            }),
            SealLaneProbe {
                armed: true,
                vad_available: true,
            },
            measured_at,
        )
        .expect("immediate readiness probe sees the persisted profile");
        assert_eq!(
            grant.calibration_version.as_deref(),
            Some("cal2-fixture-mic-10000@48000hz")
        );
    }

    /// C15D-A lifecycle falsifier: an unfinished delayed hold owns its selected
    /// generation, so refresh defers and leaves the Arc untouched.
    #[tokio::test]
    async fn pending_hold_rejects_refresh_and_preserves_generation() {
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let pending = tokio::spawn(async move {
            let _ = wait.await;
        });
        *controller.hold_start_task.lock().await = Some(pending);

        assert!(
            !controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        let after = controller.runtime_settings_arc().await;
        assert!(Arc::ptr_eq(&before, &after));

        let _ = release.send(());
        controller.cancel_pending_hold_start().await;
    }

    /// C15D-A stale-handle falsifier: a completed delayed task is cleanup, not
    /// live ownership, and therefore cannot block an idle refresh forever.
    #[tokio::test]
    async fn finished_hold_handle_does_not_block_idle_refresh() {
        let controller = RecordingController::new_without_keychain();
        let finished = tokio::spawn(async {});
        while !finished.is_finished() {
            tokio::task::yield_now().await;
        }
        *controller.hold_start_task.lock().await = Some(finished);

        assert!(
            controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        assert!(controller.hold_start_task.lock().await.is_none());
    }

    /// C15D-A active-take falsifier: both recording states keep their current
    /// Arc until the existing serialized stop/finalize path returns to Idle.
    #[tokio::test]
    async fn active_hold_and_toggle_reject_refresh() {
        let controller = RecordingController::new_without_keychain();
        for active_state in [State::RecHold, State::RecToggle] {
            *controller.state.write().await = active_state;
            let before = controller.runtime_settings_arc().await;
            assert!(
                !controller
                    .refresh_runtime_settings_from_disk()
                    .await
                    .unwrap()
            );
            let after = controller.runtime_settings_arc().await;
            assert!(Arc::ptr_eq(&before, &after));
        }
        *controller.state.write().await = State::Idle;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn deferred_settings_refresh_is_consumed_at_next_idle_selection() {
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = DataDirGuard::install(temp.path());
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        *controller.state.write().await = State::RecToggle;
        let mut settings = UserSettings::load();
        settings.paste_mode = Some(PasteMode::Off);
        settings.save().unwrap();
        assert!(
            !controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        assert!(Arc::ptr_eq(
            &before,
            &controller.runtime_settings_arc().await
        ));
        assert!(
            controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );

        // Exercise the serialized selection boundary without opening a microphone.
        let _serial_guard = controller.serial_lock.lock().await;
        *controller.state.write().await = State::Idle;
        controller
            .refresh_pending_runtime_settings_locked()
            .await
            .unwrap();
        let after = controller.runtime_settings_arc().await;
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.values().paste_mode, PasteMode::Off);
        assert!(
            !controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );
        controller
            .refresh_pending_runtime_settings_locked()
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &after,
            &controller.runtime_settings_arc().await
        ));
    }

    /// C15D-A source falsifier: each start body selects exactly one Arc under
    /// `serial_lock`, then derives config, UserSettings and recorder binding
    /// from that named Arc. This is structural evidence, not a scheduler proof.
    #[test]
    fn hold_and_toggle_each_derive_take_facts_from_one_selected_arc() {
        let source = include_str!("mod.rs");
        let hold_signature = ["async fn schedule_hold_", "start"].concat();
        let toggle_signature = ["async fn start_toggle_", "recording"].concat();
        let stop_signature = ["async fn stop_toggle_", "and_adjudicate"].concat();
        let hold_start = source.find(&hold_signature).expect("hold start body");
        let toggle_start = source.find(&toggle_signature).expect("toggle start body");
        let stop_start = source[toggle_start..]
            .find(&stop_signature)
            .map(|offset| toggle_start + offset)
            .expect("toggle stop boundary");
        let hold_body = &source[hold_start..toggle_start];
        let toggle_body = &source[toggle_start..stop_start];

        let serial_lock = ["self.serial_lock", ".lock().await"].concat();
        let select_arc = ["self.runtime_settings_", "arc().await"].concat();
        let config_projection = ["runtime_settings", ".values()"].concat();
        let user_projection = ["runtime_settings", ".user_settings()"].concat();
        let recorder_binding = ["Arc::clone(&runtime_", "settings)"].concat();

        for body in [hold_body, toggle_body] {
            assert_eq!(body.matches(&select_arc).count(), 1);
            assert_eq!(body.matches(&config_projection).count(), 1);
            assert_eq!(body.matches(&user_projection).count(), 1);
            assert_eq!(body.matches(&recorder_binding).count(), 1);
            assert!(
                body.find(&serial_lock).expect("lifecycle lock")
                    < body.find(&select_arc).expect("settings Arc selection")
            );
        }
    }

    /// C15D falsifier: profile publish uses one snapshot's values and settings.
    #[test]
    fn recording_profile_uses_controller_snapshot_user_settings() {
        let snapshot =
            Config::load_runtime_snapshot_without_keychain().expect("seal runtime settings");
        let enabled =
            apply_runtime_transcription_profile(snapshot.values(), snapshot.user_settings(), false);
        assert_eq!(enabled, snapshot.values().transcription_overlay_enabled);
    }
}

#[cfg(test)]
mod hold_start_terminal_lifecycle_falsifiers {
    use super::*;
    use crate::presentation::transcript_bus::{
        CleanTranscriptEvent, TRANSCRIPT_BUS_PATH_ENV, TranscriptSessionEndReason,
    };

    fn hold_input(action: HotkeyAction) -> HotkeyInput {
        HotkeyInput {
            key_type: HotkeyType::Hold,
            action,
            assistive: true,
            hold_mode: HoldMode::Chat,
            force_raw: false,
            force_ai: false,
        }
    }

    /// W2 P0 falsifier: key-up lands after `session_started` and before
    /// `RecHold`. The recorder mutex is the deterministic gate — the task
    /// cannot reach `TranscriptBus::open` while the test holds it, and there is
    /// no generation check between that gate and the post-start check, so a
    /// key-up issued while the gate is held and then releasing it drives exactly
    /// the audited interleaving. Virtual time only; no wall-clock sleeps.
    #[tokio::test(start_paused = true)]
    async fn keyup_between_bus_start_and_rec_hold_ends_the_session_exactly_once() {
        const CHILD: &str = "CODESCRIBE_HOLD_START_FIXTURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Follow tests/logging_isolation.rs: change only the child's env.
            // The parent's prior Bus path survives even if the fixture panics;
            // current-thread Tokio and serial_test alone cannot isolate env.
            let temp = tempfile::tempdir().expect("temp bus dir");
            let output = std::process::Command::new(
                std::env::current_exe().expect("resolve controller test binary"),
            )
            .args([
                "--exact",
                "controller::tests::hold_start_terminal_lifecycle_falsifiers::keyup_between_bus_start_and_rec_hold_ends_the_session_exactly_once",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(
                TRANSCRIPT_BUS_PATH_ENV,
                temp.path().join("transcript-events.jsonl"),
            )
            .output()
            .expect("launch isolated controller fixture");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "controller fixture failed: {stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("1 passed; 0 failed; 0 ignored"),
                "the exact controller fixture must execute: {stdout}\n{stderr}"
            );
            return;
        }
        let bus_path = std::path::PathBuf::from(
            std::env::var_os(TRANSCRIPT_BUS_PATH_ENV).expect("isolated child Bus path"),
        );

        let controller = Arc::new(RecordingController::new_without_keychain());
        let recorder_gate = controller.recorder.lock().await;
        assert!(
            recorder_gate.is_some(),
            "falsifier needs a constructed recorder to reach the Bus start"
        );
        *controller.hold_mode.write().await = HoldMode::Chat;

        controller
            .handle_hold_event(
                hold_input(HotkeyAction::Down),
                Some(controller.hold_start_generation.load(Ordering::SeqCst)),
            )
            .await
            .expect("hold down schedules a delayed start");
        let task = controller
            .hold_start_task
            .lock()
            .await
            .take()
            .expect("delayed hold start scheduled");
        let scheduled_generation = controller.hold_start_generation.load(Ordering::SeqCst);

        // Storage admission now awaits a blocking worker between the raised
        // start guard and session publication. Reach the published session,
        // not merely that earlier flag, before exercising this interleaving.
        // The held recorder mutex still excludes Bus opening and RecHold.
        while !controller.start_transition_in_flight.load(Ordering::SeqCst)
            || controller.session_id.read().await.is_none()
        {
            assert!(
                !task.is_finished(),
                "hold start finished before reaching the recorder gate"
            );
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.session_id.read().await.is_some());
        assert!(controller.active_transcript_bus.read().await.is_none());

        // The real key-up path: state is still Idle, so this cancels the
        // pending start by bumping the generation — without `serial_lock`.
        controller
            .handle_hold_event(hold_input(HotkeyAction::Up), None)
            .await
            .expect("key-up while idle cancels");
        assert_ne!(
            controller.hold_start_generation.load(Ordering::SeqCst),
            scheduled_generation
        );

        drop(recorder_gate);
        task.await.expect("hold start task joins");

        let lines: Vec<CleanTranscriptEvent> = std::fs::read_to_string(&bus_path)
            .expect("bus file written")
            .lines()
            .map(|line| serde_json::from_str(line).expect("bus line"))
            .collect();
        let statuses: Vec<&str> = lines.iter().map(|event| event.status.as_str()).collect();
        assert_eq!(statuses, ["session_started", "session_ended"]);
        assert_eq!(lines[0].session_id, lines[1].session_id);
        assert_eq!(
            lines[1].end_reason,
            Some(TranscriptSessionEndReason::StartSuperseded)
        );
        assert!(lines[1].text.is_empty());

        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.session_id.read().await.is_none());
        assert!(controller.active_transcript_bus.read().await.is_none());
        assert!(controller.assistive_context.read().await.is_none());
        assert!(controller.pre_overlay_frontmost_app.read().await.is_none());
        assert!(!controller.start_transition_in_flight.load(Ordering::SeqCst));
        assert!(
            !controller
                .recorder
                .lock()
                .await
                .as_ref()
                .expect("recorder")
                .recorder
                .is_active()
        );
    }

    /// Structural falsifier: exactly one controller call site writes the
    /// terminal Bus line, and the hold task installs its Bus into the active
    /// slot before any recorder start so every later exit can end it.
    #[test]
    fn controller_has_one_terminal_bus_publisher() {
        let source = include_str!("mod.rs");
        let publish_ended = [".publish_", "ended"].concat();
        assert_eq!(source.matches(&publish_ended).count(), 1);

        let hold_signature = ["async fn schedule_hold_", "start"].concat();
        let toggle_signature = ["async fn start_toggle_", "recording"].concat();
        let hold_start = source.find(&hold_signature).expect("hold start body");
        let toggle_start = source.find(&toggle_signature).expect("toggle start body");
        let hold_body = &source[hold_start..toggle_start];
        let bus_install = ["hold_session.active_transcript_bus", ".write().await = "].concat();
        let recorder_start = ["rec.start_event_", "session("].concat();
        assert!(
            hold_body
                .find(&bus_install)
                .expect("bus installed in hold body")
                < hold_body
                    .find(&recorder_start)
                    .expect("recorder start in hold body")
        );
    }
}

/// W2 recovery contracts, UNRUN. Exercise the production consumer and copy
/// owner with temporary directories; the daily history encoder is injected.
#[cfg(test)]
mod capture_failure_recovery_tests {
    use super::*;
    use codescribe_core::state::SessionTranscriptArchive;

    #[test]
    fn an_archive_failure_recovers_confirmed_prefix_and_unconfirmed_pcm_before_retention() {
        use codescribe_core::audio::recorder::CaptureArchiveError;
        // The real recovery consumer may write a recovered archive. Run it in
        // a child whose complete application store belongs to this test.
        if std::env::var_os("CS_PRIVATE_RECOVERY_CHILD").is_none() {
            let store = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["controller::tests::capture_failure_recovery_tests::an_archive_failure_recovers_confirmed_prefix_and_unconfirmed_pcm_before_retention", "--exact", "--test-threads=1"])
                .env("CS_PRIVATE_RECOVERY_CHILD", "1")
                .env("CODESCRIBE_DATA_DIR", store.path())
                .output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "isolated recovery failed: {stdout}\n{stderr}"
            );
            assert!(
                stdout.contains("1 passed; 0 failed"),
                "the real recovery test must execute: {stdout}"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("partial.wav");
        let mut writer = hound::WavWriter::create(
            &source,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [11_i16, 22] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let archive_error = CaptureArchiveError {
            cause: "injected disk failure".into(),
            source_path: Some(source.clone()),
            sample_rate: 16_000,
            captured_samples: 4,
            written_samples: 2,
            unwritten_samples: Arc::new(vec![33, 44]),
            buffer_start_offset: 3,
            retained_samples: Arc::new(vec![44]),
        };
        let error = anyhow::Error::new(CaptureStopFailure {
            session_id: Some("capture-owner".into()),
            capture_epoch: 7,
            audio_path: None,
            cause: archive_error.into(),
            task_failure: None,
        });
        let called = std::cell::Cell::new(false);
        let recovered = recover_capture_stop_failure(
            error,
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |id, path| {
                assert_eq!(id, "capture-owner");
                assert_ne!(
                    path,
                    source.as_path(),
                    "a partial source must not be retained as complete"
                );
                let reader = hound::WavReader::open(path).unwrap();
                assert_eq!(reader.spec().sample_rate, 16_000);
                let samples: Vec<i16> = reader.into_samples().collect::<Result<_, _>>().unwrap();
                assert_eq!(samples, [11, 22, 33, 44]);
                called.set(true);
                Ok(())
            },
        );
        assert!(
            called.get(),
            "a recoverable full take must reach the real retention consumer"
        );
        assert!(
            recovered.is::<CaptureStopFailure>(),
            "recovery must preserve the processing error"
        );
        assert!(
            source.is_file(),
            "recovery never destroys its partial source"
        );
    }

    fn failure(path: Option<std::path::PathBuf>) -> anyhow::Error {
        anyhow::Error::new(CaptureStopFailure {
            session_id: Some("capture-owner".into()),
            capture_epoch: 7,
            audio_path: path,
            cause: std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "original processing failure",
            )
            .into(),
            task_failure: None,
        })
    }

    #[test]
    fn matching_failure_retains_exact_audio_and_stays_a_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        let bytes = b"exact producer archive bytes";
        std::fs::write(&source, bytes).unwrap();
        let root = dir.path().join("archive");
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner:stopping"),
            (Some("capture-owner"), 7),
            |id, path| {
                retain_session_audio_at(
                    Some(id),
                    path,
                    SessionTranscriptArchive::Unavailable("processing failed"),
                    &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(
                        None, None,
                    ),
                    &root,
                    &mut None,
                    |file, transcript| {
                        use std::io::Read;
                        assert!(matches!(
                            transcript,
                            SessionTranscriptArchive::Unavailable(_)
                        ));
                        let mut archived = Vec::new();
                        file.read_to_end(&mut archived).unwrap();
                        assert_eq!(archived, bytes);
                        Some(source.clone())
                    },
                )
            },
        );
        let typed = error.downcast_ref::<CaptureStopFailure>().unwrap();
        assert_eq!(typed.audio_path.as_deref(), Some(source.as_path()));
        assert_eq!(
            typed.cause.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert!(error.downcast_ref::<TerminalSealRefused>().is_none());
        for path in [source, root.join("sessions/capture-owner.wav")] {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn mismatched_or_missing_identity_never_reaches_archive() {
        for (id, session, epoch) in [
            (Some("other-take"), Some("capture-owner"), 7),
            (Some("capture-owner"), Some("other-take"), 7),
            (Some("capture-owner"), Some("capture-owner"), 8),
            (Some("capture-owner"), Some("capture-owner"), 0),
            (None, Some("capture-owner"), 7),
            (Some("../capture-owner"), Some("capture-owner"), 7),
        ] {
            let error = recover_capture_stop_failure(
                failure(Some("unused.wav".into())),
                id,
                (session, epoch),
                |_, _| panic!("foreign evidence must not archive or deliver"),
            );
            assert!(error.to_string().contains("identity mismatch"));
            assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        }
    }

    #[test]
    fn no_archive_receipt_never_invents_a_saved_file() {
        let error = recover_capture_stop_failure(
            failure(None),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |_, _| panic!("no path exists to retain"),
        );
        assert!(
            error
                .downcast_ref::<CaptureStopFailure>()
                .unwrap()
                .audio_path
                .is_none()
        );
        assert!(error.to_string().contains("no finalized WAV receipt"));
        assert!(error.to_string().contains("committed text unavailable"));
    }

    #[test]
    fn failed_copy_and_daily_archive_preserve_source_and_original_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        std::fs::write(&source, b"source survives").unwrap();
        let root = dir.path().join("not-a-directory");
        std::fs::write(&root, b"block copies").unwrap();
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |id, path| {
                retain_session_audio_at(
                    Some(id),
                    path,
                    SessionTranscriptArchive::Unavailable("processing failed"),
                    &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(
                        None, None,
                    ),
                    &root,
                    &mut None,
                    |_, _| None,
                )
            },
        );
        assert!(error.to_string().contains("daily audio archive failed"));
        assert!(error.to_string().contains("sessions/capture-owner.wav"));
        assert!(error.to_string().contains("original processing failure"));
        assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        assert_eq!(std::fs::read(source).unwrap(), b"source survives");
        assert_eq!(std::fs::read(root).unwrap(), b"block copies");
    }

    #[test]
    fn daily_archive_failure_does_not_skip_session_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        std::fs::write(&source, b"retained despite daily bag failure").unwrap();
        let root = dir.path().join("copies");
        let result = retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("failure"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |_, _| None,
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(root.join("sessions/capture-owner.wav")).unwrap(),
            std::fs::read(source).unwrap()
        );
    }

    fn retain_fixture(source: &std::path::Path, root: &std::path::Path) -> Result<()> {
        retain_session_audio_at(
            Some("capture-owner"),
            source,
            SessionTranscriptArchive::Committed("fixture"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            root,
            &mut None,
            |_, _| Some(source.to_path_buf()),
        )
    }

    #[test]
    fn quiet_take_keeps_last_spoken_audio_and_each_session_links_its_take() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        std::fs::create_dir(&root).unwrap();
        let speech = dir.path().join("speech.wav");
        let quiet = dir.path().join("quiet.wav");
        std::fs::write(&speech, b"spoken take").unwrap();
        std::fs::write(&quiet, b"quiet take").unwrap();
        retain_session_audio_at(
            Some("speech-take"),
            &speech,
            SessionTranscriptArchive::Committed("hello"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |_, _| Some(speech.clone()),
        )
        .unwrap();
        retain_session_audio_at(
            Some("quiet-take"),
            &quiet,
            SessionTranscriptArchive::NoSpeech,
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |_, _| Some(quiet.clone()),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&speech).unwrap().ino(),
            std::fs::metadata(root.join("sessions/speech-take.wav"))
                .unwrap()
                .ino()
        );
        assert_eq!(
            std::fs::metadata(&quiet).unwrap().ino(),
            std::fs::metadata(root.join("sessions/quiet-take.wav"))
                .unwrap()
                .ino()
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"spoken take"
        );
    }

    #[test]
    fn repeated_retention_and_source_destination_hardlinks_preserve_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        let bytes = b"take bytes must never be truncated";
        std::fs::write(&source, bytes).unwrap();
        let session = root.join("sessions/capture-owner.wav");
        let alias = root.join("last_session.wav");
        std::fs::hard_link(&source, &session).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        for donor in [&source, &source, &session] {
            retain_fixture(donor, &root).unwrap();
            for path in [&source, &session, &alias] {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
        }
        let later = dir.path().join("later.wav");
        std::fs::write(&later, b"a later legitimate take").unwrap();
        retain_fixture(&later, &root).unwrap();
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert_eq!(std::fs::read(session).unwrap(), b"a later legitimate take");
        assert_eq!(std::fs::read(alias).unwrap(), b"a later legitimate take");
    }

    #[test]
    fn destination_links_are_replaced_without_writing_source_or_external_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        let external = dir.path().join("unrelated.wav");
        std::fs::write(&source, b"take").unwrap();
        std::fs::write(&external, b"unrelated").unwrap();
        let session = root.join("sessions/capture-owner.wav");
        let alias = root.join("last_session.wav");
        symlink(&external, &session).unwrap();
        symlink(&source, &alias).unwrap();
        retain_fixture(&source, &root).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"take");
        assert_eq!(std::fs::read(external).unwrap(), b"unrelated");
        assert!(std::fs::symlink_metadata(&session).unwrap().is_file());
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(session).unwrap(), b"take");
        assert_eq!(std::fs::read(alias).unwrap(), b"take");
    }

    #[test]
    fn sessions_symlink_refuses_escape_and_preserves_alias() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        let external = dir.path().join("external");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&external).unwrap();
        let unrelated = external.join("capture-owner.wav");
        std::fs::write(&unrelated, b"external bytes").unwrap();
        symlink(&external, root.join("sessions")).unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        let error = retain_fixture(&source, &root).unwrap_err();
        assert!(error.to_string().contains("sessions/capture-owner.wav"));
        assert_eq!(std::fs::read(unrelated).unwrap(), b"external bytes");
        assert_eq!(std::fs::read(source).unwrap(), b"take");
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn root_symlink_and_parent_traversal_are_refused() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let external = dir.path().join("external");
        std::fs::create_dir(&external).unwrap();
        let root = dir.path().join("archive");
        symlink(&external, &root).unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        assert!(retain_fixture(&source, &root).is_err());
        assert!(retain_fixture(&source, &external.join("../escape")).is_err());
        assert_eq!(std::fs::read_dir(external).unwrap().count(), 0);
        assert!(!dir.path().join("escape").exists());
        assert_eq!(std::fs::read(source).unwrap(), b"take");
    }

    #[test]
    fn nonregular_sources_are_refused_before_archive_or_copy() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("take.wav");
        std::fs::write(&regular, b"take").unwrap();
        let link = dir.path().join("link.wav");
        symlink(&regular, &link).unwrap();
        let fifo = dir.path().join("fifo.wav");
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a NUL-terminated path in this test's private temporary directory.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        for source in [link.as_path(), fifo.as_path(), dir.path()] {
            assert!(
                retain_session_audio_at(
                    Some("capture-owner"),
                    source,
                    SessionTranscriptArchive::Unavailable("fixture"),
                    &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(
                        None, None
                    ),
                    &dir.path().join("archive"),
                    &mut None,
                    |_, _| panic!("unsafe source reached archive")
                )
                .is_err()
            );
        }
        assert!(!dir.path().join("archive").exists());
        assert_eq!(std::fs::read(regular).unwrap(), b"take");
    }

    #[test]
    fn failed_publication_preserves_destination_and_cleans_temporary_entry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("archive");
        let blocked = root.join("sessions/capture-owner.wav");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("unrelated"), b"keep directory bytes").unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"take").unwrap();
        assert!(retain_fixture(&source, &root).is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"take");
        assert_eq!(
            std::fs::read(blocked.join("unrelated")).unwrap(),
            b"keep directory bytes"
        );
        assert!(!root.join("last_session.wav").exists());
        assert_eq!(std::fs::read_dir(root.join("sessions")).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    }

    #[test]
    fn source_path_replacement_after_open_cannot_change_copied_object() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("take.wav");
        let original = dir.path().join("original.wav");
        let root = dir.path().join("archive");
        std::fs::write(&source, b"opened take").unwrap();
        retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("fixture"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |file, _| {
                use std::io::Read;
                std::fs::rename(&source, &original).unwrap();
                std::fs::write(&source, b"replacement must not be copied").unwrap();
                let mut archived = Vec::new();
                file.read_to_end(&mut archived).unwrap();
                assert_eq!(archived, b"opened take");
                Some(source.clone())
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(original).unwrap(), b"opened take");
        assert_eq!(
            std::fs::read(source).unwrap(),
            b"replacement must not be copied"
        );
        assert_eq!(
            std::fs::read(root.join("sessions/capture-owner.wav")).unwrap(),
            b"opened take"
        );
        assert!(!root.join("last_session.wav").exists());
    }

    #[test]
    fn parent_and_leaf_replacement_after_open_stays_in_pinned_directories() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"opened take").unwrap();
        let root = dir.path().join("archive");
        let moved = dir.path().join("moved-archive");
        let external = dir.path().join("external");
        std::fs::create_dir(&external).unwrap();
        let unrelated = external.join("capture-owner.wav");
        std::fs::write(&unrelated, b"external bytes").unwrap();
        retain_session_audio_at(
            Some("capture-owner"),
            &source,
            SessionTranscriptArchive::Unavailable("fixture"),
            &codescribe_core::pipeline::take_truth::TakeTruth::unavailable_evidence(None, None),
            &root,
            &mut None,
            |_, _| {
                std::fs::rename(root.join("sessions"), root.join("pinned-sessions")).unwrap();
                symlink(&external, root.join("sessions")).unwrap();
                symlink(&unrelated, root.join("pinned-sessions/capture-owner.wav")).unwrap();
                symlink(&source, root.join("last_session.wav")).unwrap();
                std::fs::rename(&root, &moved).unwrap();
                symlink(&external, &root).unwrap();
                Some(source.to_path_buf())
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"external bytes");
        assert_eq!(std::fs::read_dir(external).unwrap().count(), 1);
        assert_eq!(std::fs::read(source).unwrap(), b"opened take");
        assert_eq!(
            std::fs::read(moved.join("pinned-sessions/capture-owner.wav")).unwrap(),
            b"opened take"
        );
        assert_eq!(
            std::fs::read(moved.join("last_session.wav")).unwrap(),
            b"opened take"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn recovery_copy_failure_reaches_visible_warning_and_failed_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let controller = RecordingController::new_without_keychain();
        let mut events = controller.subscribe_events();
        let bus_path = dir.path().join("bus.jsonl");
        let bus = TranscriptBus::open_at(
            TranscriptSession {
                session_id: "capture-owner".into(),
                mode: TranscriptMode::Dictation,
                has_latched_target: false,
                latched_target_is_self: false,
                audience: None,
                badge_only: false,
            },
            bus_path.clone(),
            None,
        )
        .unwrap();
        bus.publish_started();
        *controller.active_transcript_bus.write().await = Some(Arc::new(bus));
        let source = dir.path().join("take.wav");
        std::fs::write(&source, b"failed take survives").unwrap();
        let root = dir.path().join("blocked-root");
        std::fs::write(&root, b"unrelated root bytes").unwrap();
        let error = recover_capture_stop_failure(
            failure(Some(source.clone())),
            Some("capture-owner"),
            (Some("capture-owner"), 7),
            |_, path| retain_fixture(path, &root),
        );
        assert!(error.downcast_ref::<CaptureStopFailure>().is_some());
        assert_eq!(std::fs::read(source).unwrap(), b"failed take survives");
        assert_eq!(std::fs::read(root).unwrap(), b"unrelated root bytes");
        let result = Err(error);
        controller.reset_finished_recording_state(&result).await;
        controller
            .handle_processed_recording_result(false, &result)
            .await;
        let mut visible = false;
        while let Ok(event) = events.try_recv() {
            if let IpcEventPayload::Engine(EngineEventWire::Warning { code, message }) =
                event.payload
                && code == "transcription_failed"
            {
                assert!(message.contains("audio retention failed"));
                assert!(message.contains("original processing failure"));
                visible = true;
            }
        }
        assert!(visible);
        let rows: Vec<crate::presentation::transcript_bus::CleanTranscriptEvent> =
            std::fs::read_to_string(bus_path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[1].end_reason,
            Some(TranscriptSessionEndReason::TranscriptionFailed)
        );
        assert_eq!(controller.current_state().await, State::Idle);
    }
}

#[cfg(test)]
mod explicit_startup_tests {
    use super::*;
    use crate::config::{CapturedRuntimeInputs, StartupAcquisitionProbe};

    fn snapshot(root: &std::path::Path) -> RuntimeSettingsSnapshot {
        Config::runtime_snapshot_from_captured(CapturedRuntimeInputs::defaults_at(
            root.to_path_buf(),
            1_700_000_000_000,
        ))
    }

    #[tokio::test]
    async fn archive_send_cannot_consume_live_context_or_delivery_disposition() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        *controller.assistive_context.write().await = Some(AssistiveContext {
            frontmost_app: Some("Live B app".into()),
            selected_text: Some("Live B selection".into()),
        });
        *controller.pending_assistive_context.write().await = Some(AssistiveContext {
            frontmost_app: Some("Next C app".into()),
            selected_text: Some("Next C selection".into()),
        });
        controller
            .context_bucket
            .lock()
            .await
            .add_selection(0, "Live bucket B".into())
            .unwrap();
        let bucket_before = format!("{:?}", controller.context_bucket.lock().await);
        *controller.delivery_disposition.write().await = TranscriptDelivery::ComposerPending;
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
        assert!(
            controller
                .deliver_archived_transcript_to_agent_with(
                    "Archive A words".into(),
                    |text, language, tokens, explicit| {
                        Box::pin(async move {
                            sent_tx.send((text, language, tokens, explicit)).unwrap();
                        })
                    }
                )
                .await
                .unwrap()
        );
        let (text, language, tokens, explicit) = sent_rx.await.unwrap();
        assert!(text.contains("Archive A words"));
        assert!(!text.contains("Live B selection"));
        assert!(!text.contains("Next C selection"));
        assert!(!text.contains("Live bucket B"));
        let config = controller.get_config().await;
        assert_eq!(language, config.whisper_language);
        assert_eq!(tokens, config.ai_assistive_max_tokens);
        assert!(explicit);
        assert_eq!(
            controller
                .assistive_context
                .read()
                .await
                .as_ref()
                .unwrap()
                .selected_text
                .as_deref(),
            Some("Live B selection")
        );
        assert_eq!(
            controller
                .pending_assistive_context
                .read()
                .await
                .as_ref()
                .unwrap()
                .selected_text
                .as_deref(),
            Some("Next C selection")
        );
        assert_eq!(
            format!("{:?}", controller.context_bucket.lock().await),
            bucket_before
        );
        assert_eq!(
            *controller.delivery_disposition.read().await,
            TranscriptDelivery::ComposerPending
        );
        assert!(probe.attempts().is_empty());
    }

    #[tokio::test]
    async fn capture_shutdown_settled_requires_conversation_task_completion() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        assert!(!controller.capture_shutdown_settled());
        controller.request_capture_shutdown();
        assert!(controller.conversation_task.lock().await.is_none());
        assert!(controller.capture_shutdown_settled(), "no task is settled");

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            finish_rx.await.unwrap();
        });
        let task_id = task.id();
        *controller.conversation_task.lock().await = Some(task);
        started_rx.await.unwrap();
        assert!(
            !controller.capture_shutdown_settled(),
            "a running task must block shutdown settlement"
        );

        finish_tx.send(()).unwrap();
        let mut conversation = controller.conversation_task.lock().await;
        let task = conversation.as_mut().unwrap();
        // Await by reference so the completed handle remains in the owner slot.
        (&mut *task).await.unwrap();
        assert!(task.is_finished());
        assert_eq!(task.id(), task_id);
        assert!(
            !controller.capture_shutdown_settled(),
            "even a finished task cannot bypass the conversation lock"
        );
        drop(conversation);
        assert!(
            controller.capture_shutdown_settled(),
            "a finished retained task is settled"
        );
        assert_eq!(
            controller
                .conversation_task
                .lock()
                .await
                .as_ref()
                .unwrap()
                .id(),
            task_id,
            "settlement must not remove the finished handle"
        );
        assert!(probe.attempts().is_empty());
    }

    #[tokio::test]
    async fn idle_restart_reservation_refuses_active_capture_without_closing_admission() {
        let root = tempfile::tempdir().unwrap();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        for state in [State::RecHold, State::RecToggle, State::Conversation] {
            *controller.state.write().await = state;
            assert!(!controller.reserve_idle_restart());
            assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        }
        *controller.state.write().await = State::Idle;
        *controller.session_id.write().await = Some("retained-terminal-owner".into());
        assert!(!controller.reserve_idle_restart());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        *controller.session_id.write().await = None;
        assert!(controller.reserve_idle_restart());
    }

    #[tokio::test]
    async fn idle_restart_reservation_refuses_start_and_channel_lock_contention() {
        let root = tempfile::tempdir().unwrap();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        let start = controller.serial_lock.lock().await;
        assert!(!controller.reserve_idle_restart());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        drop(start);
        let channel = controller.agent_channels.lock().await;
        assert!(!controller.reserve_idle_restart());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        drop(channel);
        assert!(controller.reserve_idle_restart());
    }

    #[tokio::test]
    async fn idle_restart_reservation_refuses_scheduled_start_and_unsettled_tail() {
        let root = tempfile::tempdir().unwrap();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        let (release, blocked) = tokio::sync::oneshot::channel::<()>();
        *controller.hold_start_task.lock().await =
            Some(tokio::spawn(async { blocked.await.unwrap() }));
        assert!(!controller.reserve_idle_restart());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        release.send(()).unwrap();
        controller
            .hold_start_task
            .lock()
            .await
            .take()
            .unwrap()
            .await
            .unwrap();
        let (release, blocked) = tokio::sync::oneshot::channel::<()>();
        controller
            .closed_capture_tails
            .lock()
            .unwrap()
            .push(tokio::spawn(async {
                blocked.await.unwrap();
            }));
        assert!(!controller.reserve_idle_restart());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        release.send(()).unwrap();
        let task = controller
            .closed_capture_tails
            .lock()
            .unwrap()
            .pop()
            .unwrap();
        task.await.unwrap();
        assert!(controller.reserve_idle_restart());
    }

    #[tokio::test]
    async fn idle_restart_reservation_closes_future_dictation_conversation_and_channel_starts() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        assert!(controller.reserve_idle_restart());
        assert!(
            !controller.reserve_idle_restart(),
            "only one restart can reserve the process"
        );
        assert!(matches!(
            controller
                .start_toggle_recording(false, CaptureTurnIntent::HandsFree)
                .await
                .unwrap(),
            CaptureAdmission::NotAdmitted
        ));
        assert!(controller.start_conversation_mode().await.is_err());
        assert!(controller.toggle_agent_channel(2).await.is_err());
        assert!(controller.capture_shutdown_settled());
        assert!(
            probe.attempts().is_empty(),
            "no capture resources may start after reservation"
        );
    }

    #[tokio::test]
    async fn explicit_assembly_keeps_real_idle_lifecycle_and_passed_context_root() {
        let root = tempfile::tempdir().unwrap();
        let probe = StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            snapshot(root.path()),
            ControllerStartupResources::inert(),
            root.path(),
        );
        assert_eq!(controller.current_state().await, State::Idle);
        assert!(controller.recorder.lock().await.is_none());
        assert!(!controller.shutdown_requested.load(Ordering::SeqCst));
        let bucket = controller.context_bucket.lock().await;
        let expected = ContextBucket::for_codescribe_data_dir(root.path());
        assert_eq!(format!("{bucket:?}"), format!("{expected:?}"));
        assert!(
            !root.path().join("context").exists(),
            "assembly must not create context storage"
        );
        assert!(probe.attempts().is_empty());
    }

    #[test]
    fn normal_constructor_shared_path_still_invokes_resource_acquisition() {
        let probe = StartupAcquisitionProbe::forbid();
        let result = std::panic::catch_unwind(|| {
            RecordingController::with_runtime_settings(
                snapshot(std::path::Path::new("/fixture/normal")),
                "fixture adapter witness",
            )
        });
        assert!(result.is_err());
        assert_eq!(probe.attempts(), ["controller startup resources"]);
    }

    #[test]
    fn both_normal_constructors_still_invoke_core_host_capture() {
        for constructor in [
            RecordingController::new,
            RecordingController::new_without_keychain,
        ] {
            let probe = StartupAcquisitionProbe::forbid();
            assert!(std::panic::catch_unwind(constructor).is_err());
            assert_eq!(probe.attempts(), ["settings capture"]);
        }
    }
}

#[cfg(test)]
mod max_start_order_tests {
    #[test]
    fn recording_selection_does_not_discover_tools_before_audio_opens() {
        let controller = include_str!("mod.rs");
        let selection = controller
            .split("async fn selected_max_consultation(")
            .nth(1)
            .expect("Max selection exists")
            .split("pub fn subscribe_max_approval_changes")
            .next()
            .expect("selection body exists");
        let discovery = selection
            .find("configured_registry()")
            .expect("selected consultation uses the production registry");
        assert!(
            selection[..discovery].rfind("Box::new(||").is_some(),
            "recording selection constructs the tool registry before opening audio"
        );
    }
}

#[cfg(test)]
mod formatter_revision_request_tests {
    use super::*;
    use codescribe_core::config::CapturedRuntimeInputs;
    use std::io::Write;

    fn snapshot(root: &std::path::Path, level: FormattingPolicy) -> RuntimeSettingsSnapshot {
        let mut inputs = CapturedRuntimeInputs::defaults_at(root.to_path_buf(), 1);
        inputs.user_settings.formatting_level = Some(level.as_str().to_string());
        inputs.settings_bytes = std::fs::read(root.join("settings.json")).ok();
        Config::runtime_snapshot_from_captured(inputs)
    }

    #[test]
    fn absent_request_uses_each_settings_level() {
        let root = tempfile::tempdir().unwrap();
        for level in FormattingPolicy::ALL {
            let settings = snapshot(root.path(), level);
            assert_eq!(
                formatter_revision_level(None, &settings).unwrap(),
                (level, "settings")
            );
            assert_eq!(
                formatter_revision_level(Some(level), &settings).unwrap(),
                (level, "request")
            );
            assert_eq!(settings.formatting_policy(), level);
        }
    }

    #[test]
    fn max_request_uses_max_without_writing_settings() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let original = b"{\n  \"formatting_level\": \"smart\"\n}\n";
        std::fs::write(&path, original).unwrap();
        let settings = snapshot(root.path(), FormattingPolicy::Smart);
        let selected = formatter_revision_level(Some(FormattingPolicy::Max), &settings);
        assert_eq!(settings.formatting_policy(), FormattingPolicy::Smart);
        assert_eq!(std::fs::read(path).unwrap(), original);
        // Expected RED until the core formatter can consume a request policy
        // and its matching prompt without changing the settings snapshot.
        assert_eq!(selected.unwrap(), (FormattingPolicy::Max, "request"));
    }

    #[tokio::test]
    async fn max_request_snapshot_keeps_controller_settings() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let original = b"{\"formatting_level\":\"smart\"}\n";
        std::fs::write(&path, original).unwrap();
        let settings = snapshot(root.path(), FormattingPolicy::Smart);
        let probe = codescribe_core::config::StartupAcquisitionProbe::forbid();
        let controller = RecordingController::from_startup_inputs(
            settings,
            ControllerStartupResources::inert(),
            root.path(),
        );
        let before = controller.runtime_settings_arc().await;
        let request = controller
            .formatter_revision_settings(Some(FormattingPolicy::Max))
            .await;
        assert_eq!(request.formatting_policy(), FormattingPolicy::Max);
        assert_eq!(
            request
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .unwrap()
                .composed_content(),
            codescribe_core::config::DEFAULT_MAX_FORMATTING_PROMPT
        );
        assert_ne!(request.digest(), before.digest());
        let after = controller.runtime_settings_arc().await;
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.formatting_policy(), FormattingPolicy::Smart);
        assert_eq!(
            after
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .unwrap()
                .composed_content(),
            codescribe_core::config::DEFAULT_SMART_FORMATTING_PROMPT
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(probe.attempts().is_empty());
    }

    #[tokio::test]
    async fn absent_request_reuses_controller_snapshot() {
        let root = tempfile::tempdir().unwrap();
        for level in FormattingPolicy::ALL {
            let controller = RecordingController::from_startup_inputs(
                snapshot(root.path(), level),
                ControllerStartupResources::inert(),
                root.path(),
            );
            let before = controller.runtime_settings_arc().await;
            let request = controller.formatter_revision_settings(None).await;
            assert!(Arc::ptr_eq(&before, &request));
            assert_eq!(request.formatting_policy(), level);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_save_during_active_take_preserves_running_generation_then_updates_next_request_snapshot()
     {
        use codescribe_core::config::prompts::{
            PromptKind, PromptSource, PromptWriteReason, write_prompt,
        };
        use codescribe_core::test_isolation::EnvGuard;

        let temp = tempfile::tempdir().unwrap();
        let _data_dir = EnvGuard::set(
            "CODESCRIBE_DATA_DIR",
            temp.path().to_str().expect("utf-8 temp dir"),
        );
        let controller = RecordingController::new_without_keychain();
        let before = controller.runtime_settings_arc().await;
        assert_eq!(
            before
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .unwrap()
                .source(),
            PromptSource::BuiltInFallback
        );

        // While a take is active, saving a custom Smart prompt defers refresh so
        // the running take's sealed generation remains untouched.
        *controller.state.write().await = State::RecToggle;
        write_prompt(
            PromptKind::FormattingSmart,
            "Custom Smart prompt v2 with explicit paragraph breaks",
            PromptWriteReason::SettingsSave,
        )
        .unwrap();
        assert!(
            !controller
                .refresh_runtime_settings_from_disk()
                .await
                .unwrap()
        );
        assert!(Arc::ptr_eq(
            &before,
            &controller.runtime_settings_arc().await
        ));
        assert!(
            controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );

        // Once the take returns to Idle, the next formatter request consumes the
        // deferred refresh and seals the saved prompt into the new snapshot.
        *controller.state.write().await = State::Idle;
        let next_request = controller
            .formatter_revision_settings(Some(FormattingPolicy::Smart))
            .await;
        let sealed_prompt = next_request
            .ai_execution()
            .formatter()
            .formatting_prompt()
            .unwrap();
        assert_eq!(sealed_prompt.source(), PromptSource::CustomFile);
        assert!(
            sealed_prompt
                .composed_content()
                .contains("Custom Smart prompt v2 with explicit paragraph breaks")
        );
        assert!(
            !controller
                .runtime_settings_refresh_pending
                .load(Ordering::SeqCst)
        );
    }

    #[derive(Clone, Default)]
    struct ReceiptLog(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for ReceiptLog {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ReceiptLog {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn formatter_log_names_level_and_source_without_changing_reducer_receipt() {
        let root = tempfile::tempdir().unwrap();
        let settings = snapshot(root.path(), FormattingPolicy::Smart);
        for source in ["request", "settings"] {
            let log = ReceiptLog::default();
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_writer(log.clone())
                .finish();
            let receipt = UserRevisionCommit {
                session_id: "formatter-test".into(),
                source_revision: 4,
                revision: 5,
                rendered_text: "formatted words".into(),
                provenance_receipt: "formatter-test-4-5-0".into(),
            };
            tracing::subscriber::with_default(subscriber, || {
                log_formatter_revision(&receipt, FormattingPolicy::Smart, source, &settings);
            });
            let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert!(text.contains("format_level=\"smart\""));
            assert!(text.contains(&format!("source=\"{source}\"")));
            assert!(text.contains("prompt_source=\"built_in_fallback\""));
            assert!(text.contains("prompt_sha256="));
            assert!(text.contains("settings_sha256="));
            assert!(text.contains("provenance_receipt=formatter-test-4-5-0"));
            assert_eq!(receipt.provenance_receipt, "formatter-test-4-5-0");
            assert_eq!(receipt.source_revision, 4);
            assert_eq!(receipt.revision, 5);
        }
    }
}

#[cfg(all(test, unix))]
mod truth_pair_regression_tests {
    use super::*;
    use codescribe_core::pipeline::take_truth::{
        TakeTruth, truth_sidecar_path, write_truth_sidecar,
    };
    use codescribe_core::state::SessionTranscriptArchive;

    fn card(id: &str) -> TakeTruth {
        TakeTruth::unavailable_evidence(Some(id.to_string()), Some(7))
    }

    #[test]
    fn truth_pair_latest_alias_reads_current_card_not_old_regular_file() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let old = br#"{"take":"old"}"#;
        std::fs::write(root.join("last_session.wav.truth.json"), old).unwrap();
        let current = card("current-take");
        let bytes = serde_json::to_vec_pretty(&current).unwrap();
        let stable = root.join("sessions/current-take.wav.truth.json");
        std::fs::write(&stable, &bytes).unwrap();
        let source = d.path().join("audio.wav");
        std::fs::write(&source, b"full current audio").unwrap();
        let mut owned_card = None;
        retain_session_audio_at(
            Some("current-take"),
            &source,
            SessionTranscriptArchive::Committed("current"),
            &current,
            &root,
            &mut owned_card,
            |_, _| Some(source.clone()),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"full current audio"
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav.truth.json")).unwrap(),
            bytes
        );
        assert_eq!(owned_card, Some(stable));
        let stale: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|p| {
                let p = p.unwrap().path();
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .ends_with(".stale.json")
                    .then_some(p)
            })
            .collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(
            std::fs::read(&stale[0]).unwrap(),
            old,
            "historical evidence must survive byte-for-byte"
        );
    }

    #[test]
    fn truth_pair_late_old_stable_writer_cannot_poison_latest_card() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir(root.join("sessions")).unwrap();
        let directory = std::fs::File::open(root).unwrap();
        let mut newest = None;
        for id in ["older-take", "newer-take"] {
            let audio = root.join(format!("sessions/{id}.wav"));
            std::fs::write(&audio, id).unwrap();
            let truth = card(id);
            let bytes = serde_json::to_vec_pretty(&truth).unwrap();
            std::fs::write(truth_sidecar_path(&audio), &bytes).unwrap();
            newest = Some(bytes);
            publish_latest_capture_aliases(&directory, id, true).unwrap();
        }
        std::fs::write(
            root.join("sessions/older-take.wav.truth.json"),
            b"late CLI result for old capture",
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"newer-take"
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav.truth.json")).unwrap(),
            newest.unwrap()
        );
        let current = root.join("sessions/newer-take.wav");
        let before = std::fs::read(truth_sidecar_path(&current)).unwrap();
        assert!(
            write_truth_sidecar(&current, &TakeTruth::unavailable_evidence(None, None)).is_err()
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav.truth.json")).unwrap(),
            before,
            "file analysis cannot erase current live receipts"
        );
    }

    #[test]
    fn truth_pair_quiet_take_keeps_last_spoken_pair_and_full_audio() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let speech = d.path().join("speech.wav");
        let quiet = d.path().join("quiet.wav");
        std::fs::write(&speech, b"full speech").unwrap();
        std::fs::write(&quiet, b"full quiet").unwrap();
        let spoken_card = card("spoken-take");
        let quiet_card = card("silent-take");
        for (id, path, text, observer) in [
            (
                "spoken-take",
                &speech,
                SessionTranscriptArchive::Committed("spoken"),
                &spoken_card,
            ),
            (
                "silent-take",
                &quiet,
                SessionTranscriptArchive::NoSpeech,
                &quiet_card,
            ),
        ] {
            let mut owned_card = None;
            retain_session_audio_at(
                Some(id),
                path,
                text,
                observer,
                &root,
                &mut owned_card,
                |_, _| Some(path.clone()),
            )
            .unwrap();
            assert_eq!(
                owned_card,
                Some(root.join(format!("sessions/{id}.wav.truth.json")))
            );
        }
        assert_eq!(
            std::fs::read(root.join("sessions/silent-take.wav")).unwrap(),
            b"full quiet"
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"full speech"
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav.truth.json")).unwrap(),
            serde_json::to_vec_pretty(&spoken_card).unwrap()
        );
    }

    #[test]
    fn truth_pair_failed_export_preserves_full_audio_and_does_not_publish_old_card() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let old = br#"{"take":"old"}"#;
        std::fs::write(root.join("last_session.wav.truth.json"), old).unwrap();
        let blocker = root.join("sessions/current-take.wav.truth.json");
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("owned-file"), b"preserve special destination").unwrap();
        let source = d.path().join("source.wav");
        std::fs::write(&source, b"entire failed-export capture").unwrap();
        let mut exported = None;
        retain_session_audio_at(
            Some("current-take"),
            &source,
            SessionTranscriptArchive::Committed("current"),
            &card("current-take"),
            &root,
            &mut exported,
            |_, _| Some(source.clone()),
        )
        .unwrap();
        assert_eq!(exported, None);
        assert!(
            !root.join("last_session.wav.truth.json").exists(),
            "old card cannot masquerade as current"
        );
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"entire failed-export capture"
        );
        assert_eq!(
            std::fs::read(root.join("last_session.wav")).unwrap(),
            b"entire failed-export capture"
        );
        assert_eq!(
            std::fs::read(blocker.join("owned-file")).unwrap(),
            b"preserve special destination"
        );
    }
}

#[cfg(all(test, unix))]
mod truth_retention_parent_binding_tests {
    use super::*;
    use codescribe_core::pipeline::take_truth::{TakeTruth, truth_sidecar_path};
    use codescribe_core::state::SessionTranscriptArchive;
    #[test]
    fn take_observer_stays_beside_the_audio_after_retention_parent_rebind() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("data");
        let detached = d.path().join("detached");
        let outside = d.path().join("unrelated");
        for path in [&root, &outside] {
            std::fs::create_dir_all(path.join("sessions")).unwrap();
        }
        let source = d.path().join("source.wav");
        std::fs::write(&source, b"complete capture audio").unwrap();
        let unrelated_card = outside.join("sessions/current-take.wav.truth.json");
        let old = TakeTruth::unavailable_evidence(None, None);
        let old_bytes = serde_json::to_vec_pretty(&old).unwrap();
        std::fs::write(&unrelated_card, &old_bytes).unwrap();
        let card = TakeTruth::unavailable_evidence(Some("current-take".into()), Some(7));
        let mut stable_card = None;
        retain_session_audio_at(
            Some("current-take"),
            &source,
            SessionTranscriptArchive::Committed("current"),
            &card,
            &root,
            &mut stable_card,
            |_, _| {
                std::fs::rename(&root, &detached).unwrap();
                std::os::unix::fs::symlink(&outside, &root).unwrap();
                Some(source.clone())
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&unrelated_card).unwrap(),
            old_bytes,
            "retention observer must use the audio parent's admitted descriptor"
        );
        assert_eq!(
            std::fs::read(detached.join("sessions/current-take.wav")).unwrap(),
            b"complete capture audio"
        );
        assert_eq!(
            std::fs::read(truth_sidecar_path(
                &detached.join("sessions/current-take.wav")
            ))
            .unwrap(),
            serde_json::to_vec_pretty(&card).unwrap()
        );
        assert_eq!(
            std::fs::read(detached.join("last_session.wav.truth.json")).unwrap(),
            serde_json::to_vec_pretty(&card).unwrap()
        );
    }
}

#[cfg(test)]
mod closed_take_publication_tests {
    use super::*;
    use crate::presentation::transcript_bus::CleanTranscriptEvent;
    use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};

    struct HeldPublication {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        published: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for HeldPublication {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl EventSink for HeldPublication {
        fn on_event(&self, _: &EngineEvent) {}

        fn wait_presentation_published<'a>(
            &'a self,
            session_id: &'a str,
            _: u64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
            Box::pin(async move {
                assert_eq!(session_id, "old-take");
                self.entered.notify_one();
                self.release.notified().await;
                self.published.store(true, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    fn open_bus(id: &str, path: std::path::PathBuf) -> Arc<TranscriptBus> {
        let bus = Arc::new(
            TranscriptBus::open_at(
                TranscriptSession {
                    session_id: id.into(),
                    mode: TranscriptMode::Agent,
                    has_latched_target: false,
                    latched_target_is_self: false,
                    audience: None,
                    badge_only: false,
                },
                path,
                None,
            )
            .unwrap(),
        );
        bus.publish_started();
        bus
    }

    fn events(path: &std::path::Path) -> Vec<CleanTranscriptEvent> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_publication_keeps_original_tail_and_closes_only_old_bus() {
        let root = tempfile::tempdir().unwrap();
        let old_path = root.path().join("old.jsonl");
        let new_path = root.path().join("new.jsonl");
        let controller = RecordingController::new_without_keychain();
        *controller.active_transcript_bus.write().await =
            Some(open_bus("old-take", old_path.clone()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let published = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let mut recorder = StreamingRecorder::new().unwrap();
        let mut inputs =
            codescribe_core::config::CapturedRuntimeInputs::defaults_at(root.path().to_owned(), 1);
        // This no-provider fixture exercises ledger-mode publication ownership.
        // Direct completion separately requires the real decoder acknowledgement.
        inputs.user_settings.seal_ledger_enabled = Some(true);
        recorder.bind_session_authority(
            "old-take".into(),
            Arc::new(Config::runtime_snapshot_from_captured(inputs)),
        );
        recorder.set_event_sink(Some(Arc::new(HeldPublication {
            entered: entered.clone(),
            release: release.clone(),
            published: published.clone(),
            dropped: dropped.clone(),
        })));
        // No microphone/provider is needed to falsify ownership of the exact
        // production ClosedTake. Exact PCM/archive refusal is covered separately.
        let take = recorder.detach_closed_take(false);
        let stopping =
            tokio::spawn(
                async move { try_stop_closed_take_for_terminal(take, Some("old-take")).await },
            );
        entered.notified().await;
        tokio::time::advance(std::time::Duration::from_millis(3001)).await;
        let (result, _, pending) = stopping.await.unwrap();
        assert!(result.unwrap_err().is::<CaptureStopFailure>());
        let pending = pending.expect("timeout retains the original standalone owner");
        assert!(!pending.is_settled());
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(!published.load(Ordering::SeqCst));
        assert_eq!(
            events(&old_path).len(),
            1,
            "timeout is not terminal Bus proof"
        );
        controller
            .detach_preempted_stop(pending, Some("old-take"))
            .await
            .unwrap();
        assert!(controller.active_transcript_bus.read().await.is_none());
        *controller.active_transcript_bus.write().await =
            Some(open_bus("successor", new_path.clone()));
        *controller.session_id.write().await = Some("successor".into());
        controller.set_state(State::RecToggle).await;
        entered.notified().await;
        release.notify_one();
        let tasks = std::mem::take(&mut *controller.closed_capture_tails.lock().unwrap());
        assert_eq!(tasks.len(), 1);
        for task in tasks {
            task.await.unwrap();
        }
        assert!(published.load(Ordering::SeqCst));
        assert!(dropped.load(Ordering::SeqCst));
        let old_events = events(&old_path);
        assert_eq!(
            old_events.len(),
            2,
            "one original start and one original terminal"
        );
        assert_eq!(
            old_events[1].end_reason,
            Some(TranscriptSessionEndReason::Completed)
        );
        assert_eq!(
            events(&new_path).len(),
            1,
            "old completion cannot end the successor"
        );
        assert_eq!(controller.current_state().await, State::RecToggle);
        assert_eq!(
            controller.session_id.read().await.as_deref(),
            Some("successor")
        );
        assert!(controller.active_transcript_bus.read().await.is_some());
    }
}
