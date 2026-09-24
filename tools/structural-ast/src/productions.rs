//! Closed AST productions for ownership-bearing atoms.
//!
//! Equality is syn's structural equality, ignoring spans/comments/whitespace.
//! A changed callback/loop/macro is a new proof obligation, never an opaque
//! success. The productions include argument identity and binding provenance:
//! merely spelling `focus_confirmed`, `receipt`, or a callee is insufficient.
use super::Grammar;
use syn::{Block, Stmt, parse_quote};

pub(super) fn overlay(g: &mut Grammar, body: &Block) {
    g.sequence(body, vec![
        (parse_quote!(let trimmed = text.trim();), "trim input"),
        (parse_quote!(let target_app = self.pre_overlay_frontmost_app.read().await.clone();), "read latched target"),
        (parse_quote!(let intent = DeliveryIntent::OverlayInsert;), "explicit overlay intent"),
        (parse_quote!(let decision = resolve_delivery_route(intent, overlay_insert_facts(!trimmed.is_empty(), false));), "resolve explicit route"),
        (parse_quote!(info!("{}", format_delivery_route_line(intent, decision, target_app.as_deref()));), "observational route log"),
        (parse_quote!(if trimmed.is_empty() || decision.route == DeliveryRoute::ArchiveOnly {
            return Ok(OverlayPasteResult { delivery: OverlayPasteDelivery::Noop,
                target_app_name: None, frontmost_app_name: None,
                deferred_insert_shortcut: None, deferred_insert_failure: None, });
        }), "only empty/archive early success is Noop"),
        (parse_quote!(let config = self.get_config().await;), "read immutable delivery config"),
        (parse_quote!(let payload = self.delivery_tagger.render(trimmed, &config, None);), "render delivery-only transcript tag"),
        (parse_quote!(if decision.route == DeliveryRoute::DeferredInsert {
            return self.arm_overlay_text(&payload, target_app, Some("Codescribe".to_string())).await;
        }), "deferred route return"),
        (Stmt::Expr(parse_quote!(self.execute_clipboard_paste(payload, target_app, "Overlay paste").await), None), "await guarded helper tail"),
    ]);
}

pub(super) fn paste(g: &mut Grammar, body: &Block) {
    // The guard's bindings are authenticated before accepting the conditional.
    // Closures here are exact predicate productions, not generic opaque calls.
    g.sequence(body, vec![
        (parse_quote!(let focus_confirmed = target_app.as_deref().map(str::trim)
            .filter(|name| !name.is_empty()).is_some_and(|name| {
                is_codescribe_app(name) || (crate::os::selection::activate_app_by_name(name)
                    && crate::os::selection::wait_for_frontmost_app(name, Duration::from_millis(250),))
            });), "observe target activation"),
        (parse_quote!(let frontmost = crate::os::selection::current_frontmost_app_name();), "observe frontmost"),
        (parse_quote!(let target_observed_frontmost = matches!(
            (target_app.as_deref(), frontmost.as_deref()),
            (Some(target), Some(front)) if front.trim().eq_ignore_ascii_case(target.trim())
        );), "exact matches predicate"),
        (parse_quote!(let frontmost_is_external = frontmost.as_deref().map(str::trim)
            .filter(|name| !name.is_empty()).is_some_and(|name| !is_codescribe_app(name));), "external target predicate"),
        (parse_quote!(debug!(target = ?target_app, frontmost = ?frontmost,
            focus_confirmed_by_wait = focus_confirmed, target_observed_frontmost,
            frontmost_is_external, "{context}: paste target activation");), "observational focus log"),
        (parse_quote!(let focus_confirmed = delivery_route::clipboard_paste_may_post(
            target_app.is_some(), focus_confirmed, target_observed_frontmost, frontmost_is_external,
        );), "latched focus policy binding"),
        (parse_quote!(let config = self.get_config().await;), "read deferred config"),
        (parse_quote!(let preflight = clipboard::synthetic_paste_preflight();), "preflight binding"),
        (parse_quote!(let mut deferred_insert_shortcut = None;), "initialize shortcut"),
        (parse_quote!(let mut deferred_insert_failure = None;), "initialize failure"),
        (parse_quote!(let delivery = if focus_confirmed && preflight.can_post_events() {
            clipboard::paste_and_restore(&paste_text)
                .with_context(|| format!("{context}: failed to paste"))?;
            OverlayPasteDelivery::Pasted
        } else {
            warn!(target_app = ?target_app, frontmost_app = ?frontmost,
                cg_post_event_access = preflight.cg_post_event_access,
                ax_trusted = preflight.ax_trusted, focus_confirmed,
                "{context}: could not execute the selected clipboard route; arming deferred insert");
            self.arm_or_copy_deferred_payload(paste_text, &config,
                &mut deferred_insert_shortcut, &mut deferred_insert_failure,)?
        };), "guarded effect versus deferred branch"),
        (Stmt::Expr(parse_quote!(Ok(OverlayPasteResult { delivery, target_app_name: target_app,
            frontmost_app_name: frontmost, deferred_insert_shortcut, deferred_insert_failure, })), None), "return preserved delivery result"),
    ]);
}

pub(super) fn stop(g: &mut Grammar, body: &Block) {
    g.sequence(
        body,
        vec![
            (
                parse_quote!(info!("Stopping streaming recorder...");),
                "observational stop log",
            ),
            (
                parse_quote!(let drops = self.dropped_chunks.load(Ordering::Relaxed);),
                "read drop counter",
            ),
            (
                parse_quote!(if drops > 0 {
                    warn!(
                        "Recording session: dropped {} audio chunk(s) due to backpressure",
                        drops
                    );
                }),
                "observational drop branch",
            ),
            (
                parse_quote!(let stopped = self.recorder.stop().await;),
                "await recorder; retain error without question-mark exit",
            ),
            (
                Stmt::Expr(parse_quote!(self.complete_stop(stopped).await), None),
                "unconditional await complete_stop tail",
            ),
        ],
    );
}

pub(super) fn complete(g: &mut Grammar, body: &Block) {
    // Each atom is one state transition, including BOTH paths of optional
    // ownership. No arbitrary callee is assumed to complete a shutdown step.
    let shutdown: Vec<(Stmt, &str)> = vec![
        (
            parse_quote!(if let Some(sender) = self.terminal_audio_sender.take() {
                let receipt = match &stopped {
                    Ok(Some(path)) => Ok(
                        crate::pipeline::streaming::live_audio_buffer::FinalizedPcmArchive {
                            session_id: self.authority_session_id.clone().unwrap_or_default(),
                            capture_epoch: self.capture_epoch,
                            sample_rate: self.sample_rate,
                            sample_count: self.captured_samples.load(Ordering::Relaxed),
                            path: path.clone(),
                        },
                    ),
                    Ok(None) => Err("capture finalized without a WAV archive".into()),
                    Err(error) => Err(format!("capture archive finalization failed: {error}")),
                };
                let _ = sender.send(receipt);
            }),
            "publish available archive receipt",
        ),
        (
            parse_quote!(self.lifecycle_handle = None;),
            "clear lifecycle",
        ),
        (
            parse_quote!(let task_failure = if let Some(handle) = self.transcription_handle.as_mut() {
            debug!("Waiting for transcription session task to finish...");
            handle.await.context("Transcription session task failed").err()
        } else { None };),
            "join owned task or prove absent; retain join failure",
        ),
        (
            parse_quote!(self.transcription_handle = None;),
            "clear joined task handle",
        ),
        (
            parse_quote!(if self.event_sink.is_some() {
                let drain_deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    let snapshot = self.transcript_buffer.lock().await.len();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if self.transcript_buffer.lock().await.len() == snapshot
                        || tokio::time::Instant::now() >= drain_deadline
                    {
                        break;
                    }
                }
            }),
            "drain owned sink; only loop-local break",
        ),
        (parse_quote!(self.event_sink = None;), "drop drained sink"),
        (
            parse_quote!(let (audio_path, cause, task_failure) = match stopped {
            Ok(path) => (path, task_failure, None),
            Err(error) => (None, Some(error), task_failure),
        };),
            "classify archive failure after shutdown",
        ),
        (
            parse_quote!(if let Some(cause) = cause {
                return Err(anyhow::Error::new(CaptureStopFailure {
                    session_id: self.authority_session_id.clone(),
                    capture_epoch: self.capture_epoch,
                    audio_path,
                    cause,
                    task_failure,
                }));
            }),
            "typed capture failure only after shutdown",
        ),
        (
            parse_quote!(let transcript = self.transcript_buffer.lock().await.clone();),
            "read committed transcript after owned shutdown",
        ),
        (
            parse_quote!(let empty_capture = self.captured_samples.load(Ordering::Relaxed) == 0
                && transcript.is_empty()
                && self.acoustic_ledger.as_ref().is_none_or(|ledger| {
                    ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                        .has_no_capture_facts()
                });),
            "empty capture requires zero samples and no conflicting ledger facts",
        ),
        (
            parse_quote!(if empty_capture {
                return Ok((transcript, audio_path));
            }),
            "zero-sample empty capture is not a fabricated speech seal",
        ),
        (
            parse_quote!(let finality = self.authority_session_id.as_deref().and_then(|session| {
                self.acoustic_ledger.as_ref().map(|ledger| {
                    ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
                        .terminal_finality(session, self.capture_epoch)
                })
            });),
            "inspect issued finality using exact capture identity",
        ),
        (
            Stmt::Expr(
                syn::parse_str(
                    r#"match finality {
                    Some(crate::pipeline::acoustic_ledger::TerminalFinality::Sealed(_)) => {
                        Ok((transcript, audio_path))
                    }
                    Some(crate::pipeline::acoustic_ledger::TerminalFinality::ObservedSilence(_))
                        if transcript.is_empty() => {
                        Ok((transcript, audio_path))
                    }
                    Some(crate::pipeline::acoustic_ledger::TerminalFinality::Refused(finality)) => {
                        Err(anyhow::Error::new(TerminalSealRefused {
                            finality,
                            audio_path,
                            committed_text: transcript,
                        }))
                    }
                    _ => Err(anyhow::Error::new(CaptureStopFailure {
                        session_id: self.authority_session_id.clone(),
                        capture_epoch: self.capture_epoch,
                        audio_path,
                        cause: anyhow!("recording terminal authority unavailable or inconsistent"),
                        task_failure: None,
                    })),
                }"#,
                )
                .expect("reviewed finality production"),
                None,
            ),
            "issued finality or observed empty silence succeeds; refusal retains audio and words",
        ),
    ];
    g.sequence(body, shutdown);
}
