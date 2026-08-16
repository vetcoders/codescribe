//! Delivery throne: one session, one destination, chosen by intent — never by
//! whoever happens to be frontmost at stop.
//!
//! The mic, the transcript, and the agent chain stay other thrones. This module
//! is only the destination axis (operator diagnosis 2026-08-15: "walka o tron").
//!
//! Law:
//! - `DeliveryIntent` is frozen at session start (or at an explicit overlay
//!   click). It is not re-derived from OS focus.
//! - `resolve_delivery_route` is the only function allowed to pick a
//!   [`DeliveryRoute`]. Auto-paste, overlay Insert, and To Agent consult it;
//!   they do not invent a second destination.
//! - Codescribe is never a legal Cmd+V target. A latched self-app (Agent
//!   composer / overlay / settings) routes to the Orient canvas or the Agent
//!   composer as a first-class message — never as a tagged paste into ourselves.

/// Where a finished transcript is allowed to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRoute {
    /// Spoken intent goes to the Agent composer as a first-class message.
    /// Never a clipboard paste into whatever is focused.
    AgentComposer,
    /// Transcript stays on the Orient overlay canvas. No paste, no agent send.
    OrientCanvas,
    /// Auto-paste / overlay Insert into the *latched session target*.
    /// Focus at stop time is not the authority.
    ClipboardPaste,
    /// Armed for a later explicit Paste Here. Constructed when the overlay
    /// Insert / defer click refuses a synthetic paste into Codescribe.
    DeferredInsert,
    /// History / notes / RAW only — no user-visible delivery.
    ArchiveOnly,
}

impl DeliveryRoute {
    /// Stable telemetry label (snake_case, one token).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentComposer => "agent_composer",
            Self::OrientCanvas => "orient_canvas",
            Self::ClipboardPaste => "clipboard_paste",
            Self::DeferredInsert => "deferred_insert",
            Self::ArchiveOnly => "archive_only",
        }
    }

    /// True when the stop path is allowed to post a synthetic Cmd+V.
    pub const fn posts_synthetic_paste(self) -> bool {
        matches!(self, Self::ClipboardPaste)
    }
}

/// Session-start (or explicit overlay) intent. Frozen before recording ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryIntent {
    /// Hold Fn / Globe — Orient dictation.
    OrientDictation,
    /// Double-left-option formatting hold — still Orient, may auto-paste formatted.
    OrientFormat,
    /// Assistive / Double-right-option — Agent composer is the destination.
    AgentVoice,
    /// Explicit overlay "To Agent" after any session.
    OverlayToAgent,
    /// Explicit overlay Insert / Paste Here. Frozen at the click, not at stop.
    OverlayInsert,
    /// Notes-only / save-only.
    NotesOnly,
}

impl DeliveryIntent {
    /// Stable telemetry label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OrientDictation => "orient_dictation",
            Self::OrientFormat => "orient_format",
            Self::AgentVoice => "agent_voice",
            Self::OverlayToAgent => "overlay_to_agent",
            Self::OverlayInsert => "overlay_insert",
            Self::NotesOnly => "notes_only",
        }
    }
}

/// Facts the destination function is allowed to read. Focus-at-stop is not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryFacts {
    pub has_text: bool,
    pub no_speech: bool,
    pub auto_paste_enabled: bool,
    pub overlay_enabled: bool,
    pub live_stream_session: bool,
    pub commit_required: bool,
    /// Latched pre-overlay target is Codescribe itself (Agent / overlay / settings).
    pub latched_target_is_self: bool,
}

/// One verdict: a route plus a stable reason token for the budget line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryDecision {
    pub route: DeliveryRoute,
    pub reason: &'static str,
}

/// Map session flags onto an intent. Assistive wins; notes-only next; format
/// hold is still Orient (destination is the canvas / latched target, not Agent).
pub fn delivery_intent_from_session(
    assistive: bool,
    force_ai: bool,
    notes_save_only: bool,
) -> DeliveryIntent {
    if assistive {
        DeliveryIntent::AgentVoice
    } else if notes_save_only {
        DeliveryIntent::NotesOnly
    } else if force_ai {
        DeliveryIntent::OrientFormat
    } else {
        DeliveryIntent::OrientDictation
    }
}

/// Codescribe (any chrome) is never a legal synthetic-paste target.
pub fn target_is_self_app(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("codescribe")
}

/// Facts an overlay Insert / defer click may feed the throne.
///
/// Focus-at-click is not an input. `latched_target_is_self` is true when the
/// recorded target is Codescribe, or when Swift already knows the caret is
/// still inside our chrome (`defer_text_from_overlay`).
pub fn overlay_insert_facts(has_text: bool, latched_target_is_self: bool) -> DeliveryFacts {
    DeliveryFacts {
        has_text,
        no_speech: false,
        auto_paste_enabled: false,
        overlay_enabled: true,
        live_stream_session: false,
        commit_required: false,
        latched_target_is_self,
    }
}

/// Single destination function. Advisors (quality gate, overlay flag, auto-paste
/// toggle) may veto a paste; they may not pick a different throne.
pub fn resolve_delivery_route(intent: DeliveryIntent, facts: DeliveryFacts) -> DeliveryDecision {
    if !facts.has_text || facts.no_speech {
        return DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "empty_or_no_speech",
        };
    }

    match intent {
        DeliveryIntent::AgentVoice => DeliveryDecision {
            route: DeliveryRoute::AgentComposer,
            reason: "assistive_intent",
        },
        DeliveryIntent::OverlayToAgent => DeliveryDecision {
            route: DeliveryRoute::AgentComposer,
            reason: "explicit_to_agent",
        },
        DeliveryIntent::NotesOnly => DeliveryDecision {
            route: DeliveryRoute::ArchiveOnly,
            reason: "notes_save_only",
        },
        DeliveryIntent::OverlayInsert => overlay_insert_route(facts),
        DeliveryIntent::OrientDictation | DeliveryIntent::OrientFormat => orient_route(facts),
    }
}

/// Explicit overlay click. Orient vetoes (live stream, quality commit) do not
/// apply — the user asked to insert *now*. Codescribe as the latched target
/// still refuses Cmd+V into ourselves.
fn overlay_insert_route(facts: DeliveryFacts) -> DeliveryDecision {
    if facts.latched_target_is_self {
        return DeliveryDecision {
            route: DeliveryRoute::DeferredInsert,
            reason: "refuse_paste_into_self",
        };
    }
    DeliveryDecision {
        route: DeliveryRoute::ClipboardPaste,
        reason: "explicit_insert",
    }
}

fn orient_route(facts: DeliveryFacts) -> DeliveryDecision {
    if facts.live_stream_session {
        return DeliveryDecision {
            route: DeliveryRoute::OrientCanvas,
            reason: "live_stream_owns_canvas",
        };
    }
    if facts.commit_required {
        return DeliveryDecision {
            route: DeliveryRoute::OrientCanvas,
            reason: "quality_commit_pending",
        };
    }
    if facts.latched_target_is_self {
        return DeliveryDecision {
            route: DeliveryRoute::OrientCanvas,
            reason: "refuse_paste_into_self",
        };
    }
    if facts.auto_paste_enabled {
        return DeliveryDecision {
            route: DeliveryRoute::ClipboardPaste,
            reason: "auto_paste_to_latched_target",
        };
    }
    if facts.overlay_enabled {
        return DeliveryDecision {
            route: DeliveryRoute::OrientCanvas,
            reason: "overlay_is_destination",
        };
    }
    DeliveryDecision {
        route: DeliveryRoute::ArchiveOnly,
        reason: "no_visible_surface",
    }
}

/// One INFO line: route, reason, intent, latched target. The stop-path budget
/// already has a `delivery_secs` phase; this names *where* those seconds went.
pub fn format_delivery_route_line(
    intent: DeliveryIntent,
    decision: DeliveryDecision,
    latched_target: Option<&str>,
) -> String {
    format!(
        "delivery_route: intent={intent} route={route} reason={reason} target={target}",
        intent = intent.as_str(),
        route = decision.route.as_str(),
        reason = decision.reason,
        target = latched_target.unwrap_or("-"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(overrides: impl FnOnce(&mut DeliveryFacts)) -> DeliveryFacts {
        let mut f = DeliveryFacts {
            has_text: true,
            no_speech: false,
            auto_paste_enabled: true,
            overlay_enabled: true,
            live_stream_session: false,
            commit_required: false,
            latched_target_is_self: false,
        };
        overrides(&mut f);
        f
    }

    #[test]
    fn empty_or_no_speech_archives_regardless_of_intent() {
        for intent in [
            DeliveryIntent::OrientDictation,
            DeliveryIntent::AgentVoice,
            DeliveryIntent::OverlayToAgent,
            DeliveryIntent::OverlayInsert,
        ] {
            let empty = resolve_delivery_route(
                intent,
                facts(|f| {
                    f.has_text = false;
                }),
            );
            assert_eq!(empty.route, DeliveryRoute::ArchiveOnly, "{intent:?}");
            assert_eq!(empty.reason, "empty_or_no_speech");

            let silent = resolve_delivery_route(
                intent,
                facts(|f| {
                    f.no_speech = true;
                }),
            );
            assert_eq!(silent.route, DeliveryRoute::ArchiveOnly, "{intent:?}");
        }
    }

    #[test]
    fn assistive_never_pastes() {
        let decision = resolve_delivery_route(DeliveryIntent::AgentVoice, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::AgentComposer);
        assert_eq!(decision.reason, "assistive_intent");
        assert!(!decision.route.posts_synthetic_paste());
    }

    #[test]
    fn overlay_to_agent_is_first_class_not_focus_paste() {
        let decision = resolve_delivery_route(DeliveryIntent::OverlayToAgent, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::AgentComposer);
        assert_eq!(decision.reason, "explicit_to_agent");
    }

    #[test]
    fn hold_fn_with_agent_focused_stays_on_canvas() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.latched_target_is_self = true;
                f.auto_paste_enabled = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::OrientCanvas);
        assert_eq!(decision.reason, "refuse_paste_into_self");
        assert!(!decision.route.posts_synthetic_paste());
    }

    #[test]
    fn hold_fn_auto_paste_targets_latched_app() {
        let decision = resolve_delivery_route(DeliveryIntent::OrientDictation, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(decision.reason, "auto_paste_to_latched_target");
        assert!(decision.route.posts_synthetic_paste());
    }

    #[test]
    fn overlay_without_auto_paste_is_the_canvas() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.auto_paste_enabled = false;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::OrientCanvas);
        assert_eq!(decision.reason, "overlay_is_destination");
    }

    #[test]
    fn quality_commit_and_live_stream_veto_paste() {
        let commit = resolve_delivery_route(
            DeliveryIntent::OrientFormat,
            facts(|f| {
                f.commit_required = true;
            }),
        );
        assert_eq!(commit.route, DeliveryRoute::OrientCanvas);
        assert_eq!(commit.reason, "quality_commit_pending");

        let live = resolve_delivery_route(
            DeliveryIntent::OrientDictation,
            facts(|f| {
                f.live_stream_session = true;
            }),
        );
        assert_eq!(live.route, DeliveryRoute::OrientCanvas);
        assert_eq!(live.reason, "live_stream_owns_canvas");
    }

    #[test]
    fn overlay_insert_to_foreign_app_is_clipboard_paste() {
        let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(decision.reason, "explicit_insert");
        assert!(decision.route.posts_synthetic_paste());
    }

    #[test]
    fn overlay_insert_into_self_is_deferred() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.latched_target_is_self = true;
                f.auto_paste_enabled = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::DeferredInsert);
        assert_eq!(decision.reason, "refuse_paste_into_self");
        assert!(!decision.route.posts_synthetic_paste());
    }

    #[test]
    fn overlay_insert_ignores_live_stream_and_commit_vetoes() {
        let decision = resolve_delivery_route(
            DeliveryIntent::OverlayInsert,
            facts(|f| {
                f.live_stream_session = true;
                f.commit_required = true;
            }),
        );
        assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
        assert_eq!(decision.reason, "explicit_insert");
    }

    #[test]
    fn overlay_insert_facts_are_the_click_constructor() {
        let click = overlay_insert_facts(true, true);
        assert!(!click.auto_paste_enabled);
        assert!(click.overlay_enabled);
        assert!(click.latched_target_is_self);
        let decision = resolve_delivery_route(DeliveryIntent::OverlayInsert, click);
        assert_eq!(decision.route, DeliveryRoute::DeferredInsert);
    }

    #[test]
    fn notes_only_never_pastes() {
        let decision = resolve_delivery_route(DeliveryIntent::NotesOnly, facts(|_| {}));
        assert_eq!(decision.route, DeliveryRoute::ArchiveOnly);
        assert_eq!(decision.reason, "notes_save_only");
    }

    #[test]
    fn session_flags_map_to_intent() {
        assert_eq!(
            delivery_intent_from_session(true, true, true),
            DeliveryIntent::AgentVoice
        );
        assert_eq!(
            delivery_intent_from_session(false, false, true),
            DeliveryIntent::NotesOnly
        );
        assert_eq!(
            delivery_intent_from_session(false, true, false),
            DeliveryIntent::OrientFormat
        );
        assert_eq!(
            delivery_intent_from_session(false, false, false),
            DeliveryIntent::OrientDictation
        );
    }

    #[test]
    fn codescribe_is_self_case_insensitive() {
        assert!(target_is_self_app("Codescribe"));
        assert!(target_is_self_app(" codescribe "));
        assert!(!target_is_self_app("Ghostty"));
        assert!(!target_is_self_app(""));
    }

    #[test]
    fn budget_line_names_the_throne() {
        let line = format_delivery_route_line(
            DeliveryIntent::OrientDictation,
            DeliveryDecision {
                route: DeliveryRoute::ClipboardPaste,
                reason: "auto_paste_to_latched_target",
            },
            Some("Ghostty"),
        );
        assert_eq!(
            line,
            "delivery_route: intent=orient_dictation route=clipboard_paste reason=auto_paste_to_latched_target target=Ghostty"
        );
    }
}
