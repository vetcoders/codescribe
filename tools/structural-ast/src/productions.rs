//! Closed AST productions for ownership-bearing atoms.
//!
//! Equality is syn's structural equality, ignoring spans/comments/whitespace.
//! A changed callback/loop/macro is a new proof obligation, never an opaque
//! success. The productions include argument identity and binding provenance:
//! merely spelling `focus_confirmed`, `receipt`, or a callee is insufficient.
use super::Grammar;
use syn::{Block, Stmt, parse_quote};

pub(super) fn overlay(g: &mut Grammar, body: &Block) {
    g.sequence(
        body,
        vec![
            (parse_quote!(let trimmed = text.trim();), "trim input"),
            (
                parse_quote!(let target_app = self.pre_overlay_frontmost_app.read().await.clone();),
                "read latched target",
            ),
            // `noop()` is admitted only together with its own body (see `noop`
            // below): the constructor never inherits its meaning from its name.
            (
                parse_quote!(if trimmed.is_empty() {
                    return Ok(OverlayPasteResult::noop());
                }),
                "only empty/archive early success is Noop",
            ),
            (
                parse_quote!(let config = self.get_config().await;),
                "read immutable delivery config",
            ),
            (
                parse_quote!(let payload = self.delivery_tagger.render(trimmed, &config, None);),
                "render delivery-only transcript tag",
            ),
            (
                Stmt::Expr(
                    parse_quote!(
                        self.execute_clipboard_paste(payload, target_app, "Overlay paste")
                            .await
                    ),
                    None,
                ),
                "await guarded helper tail",
            ),
        ],
    );
}

/// The overlay early return's `OverlayPasteResult::noop()`: no transport ran,
/// so the result names no delivery, no target, no frontmost app and no
/// deferred-insert outcome. Any other field value is a new proof obligation.
pub(super) fn noop(g: &mut Grammar, body: &Block) {
    let expected: Block = parse_quote!({
        Self {
            delivery: OverlayPasteDelivery::Noop,
            target_app_name: None,
            frontmost_app_name: None,
            deferred_insert_shortcut: None,
            deferred_insert_failure: None,
        }
    });
    g.require(
        body == &expected,
        "BOUNDARY: unsupported Noop result constructor: every field must stay empty",
    );
}

pub(super) fn paste(g: &mut Grammar, body: &Block) {
    // The guard's bindings are authenticated before accepting the conditional.
    // Closures here are exact predicate productions, not generic opaque calls.
    g.sequence(body, vec![
        (parse_quote!(let config = self.get_config().await;), "read config before target observation"),
        (parse_quote!(let focus_confirmed = target_app.as_deref().map(str::trim)
            .filter(|name| !name.is_empty()).is_some_and(|name| {
                is_codescribe_app(name) || (crate::os::selection::activate_app_by_name(name)
                    && crate::os::selection::wait_for_frontmost_app(name, Duration::from_millis(250),))
            });), "observe target activation"),
        (parse_quote!(let target = clipboard::StopPasteTarget::capture();), "retain recipient identity before focus confirmation"),
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
            target_app.is_some(), focus_confirmed && target_observed_frontmost,
            target_observed_frontmost, frontmost_is_external,
        ) && frontmost.as_deref().is_some_and(|name| target.matches_app_name(name));), "latched current focus and retained process binding"),
        (parse_quote!(let preflight = clipboard::synthetic_paste_preflight();), "preflight binding"),
        (parse_quote!(let mut facts = overlay_insert_facts(!paste_text.is_empty(), false);), "explicit insert facts"),
        (parse_quote!(facts.paste_target = helpers::observe_paste_target(frontmost.as_deref());), "observe input capability"),
        (parse_quote!(facts.executable_payload = looks_executable(&paste_text);), "observe terminal payload risk"),
        (parse_quote!(let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, facts);), "resolve capability gate through delivery throne"),
        (parse_quote!(info!("{}", format_delivery_route_line(
            DeliveryIntent::OverlayInsert, decision, target_app.as_deref()
        ));), "observational route log"),
        (parse_quote!(let mut deferred_insert_shortcut = None;), "initialize shortcut"),
        (parse_quote!(let mut deferred_insert_failure = None;), "initialize failure"),
        (parse_quote!(let delivery = if decision.route == DeliveryRoute::ClipboardHold {
            self.arm_or_copy_deferred_payload(paste_text.clone(), &config,
                &mut deferred_insert_shortcut, &mut deferred_insert_failure,)?
        } else if focus_confirmed && preflight.can_post_events() {
            match clipboard::paste_to_stop_target(&paste_text, &target) {
                Ok(receipt) if receipt.delivery == clipboard::StopPasteDelivery::Pasted => {
                    OverlayPasteDelivery::PasteRequested
                }
                outcome => {
                    if let Err(error) = outcome {
                        warn!(%error, "{context}: paste failed; arming deferred insert");
                    }
                    self.arm_or_copy_deferred_payload(paste_text.clone(), &config,
                        &mut deferred_insert_shortcut, &mut deferred_insert_failure,)?
                }
            }
        } else {
            warn!(target_app = ?target_app, frontmost_app = ?frontmost,
                cg_post_event_access = preflight.cg_post_event_access,
                ax_trusted = preflight.ax_trusted, focus_confirmed,
                "{context}: could not execute the selected clipboard route; arming deferred insert");
            self.arm_or_copy_deferred_payload(paste_text, &config,
                &mut deferred_insert_shortcut, &mut deferred_insert_failure,)?
        };), "capability hold versus identity guarded request versus deferred branch"),
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
                parse_quote!(let was_active = self.close_capture().await;),
                "await feed release and conditional physical close",
            ),
            (
                Stmt::Expr(
                    parse_quote!(self.finish_closed_capture(was_active).await),
                    None,
                ),
                "unconditionally settle retained closed owner using actual physical-close result",
            ),
        ],
    );
}
