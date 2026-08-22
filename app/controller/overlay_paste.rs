//! Overlay Insert / Paste Here delivery disposition and deferred-insert arming.

use std::time::Duration;

use crate::config::DeferredInsertShortcut;

use super::delivery_route::target_is_self_app;

/// Focus restore budget after activating the overlay paste target app.
pub(super) const OVERLAY_PASTE_FOCUS_BUDGET: Duration = Duration::from_millis(250);

/// Outcome of the overlay Insert action's delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPasteDelivery {
    /// Synthetic Cmd+V was posted at the restored target's caret.
    Pasted,
    /// Explicit Copy (or a leftover UniFFI outcome). Automatic refuse of
    /// Cmd+V must not take this branch — that path parks Paste Here and
    /// leaves the user's pasteboard alone.
    CopiedToClipboard,
    /// Synthetic event posting is not trusted; tagged text was copied instead.
    AccessibilityPermissionNeeded,
    /// Tagged text is armed in process memory for the global Paste Here command.
    DeferredInsertArmed,
    /// Nothing to deliver (empty transcript).
    Noop,
}

/// Full outcome of an overlay Insert action, including the context needed to
/// explain a fallback to the user.
///
/// The app-name fields are carried even on success so the UI can name the target
/// ("Pasted into Xcode"); the `deferred_insert_*` fields are populated only on
/// the [`OverlayPasteDelivery::DeferredInsertArmed`] path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPasteResult {
    pub delivery: OverlayPasteDelivery,
    pub target_app_name: Option<String>,
    pub frontmost_app_name: Option<String>,
    pub deferred_insert_shortcut: Option<String>,
    pub deferred_insert_failure: Option<String>,
}

/// Whether the global "Paste Here" hotkey is usable right now, and if not, the
/// operator-facing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeferredInsertRegistration {
    Available { shortcut_label: String },
    Unavailable { reason: String },
}

/// Decide deferred-insert availability from the three things that can block it:
/// the shortcut being disabled, the hotkey manager failing to register, or a
/// collision with an existing binding.
///
/// Every unavailable branch carries a specific reason string — the UI shows it
/// verbatim, so "it silently did nothing" is not a reachable state.
pub(super) fn deferred_insert_registration(
    shortcut: DeferredInsertShortcut,
    manager_active: bool,
    collision: Option<&str>,
) -> DeferredInsertRegistration {
    if !shortcut.is_enabled() {
        return DeferredInsertRegistration::Unavailable {
            reason: "Paste Here shortcut is disabled".to_string(),
        };
    }
    if !manager_active {
        return DeferredInsertRegistration::Unavailable {
            reason: "Paste Here hotkey registration failed".to_string(),
        };
    }
    if let Some(reason) = collision {
        return DeferredInsertRegistration::Unavailable {
            reason: reason.to_string(),
        };
    }
    DeferredInsertRegistration::Available {
        shortcut_label: shortcut.label().to_string(),
    }
}

/// Whether a synthetic paste may be posted, or which precondition failed and
/// forces the copy-to-clipboard fallback.
///
/// Each `Copy*` variant is a distinct diagnosis, not interchangeable: they tell
/// the user whether focus was lost, whether the wrong app is frontmost, or
/// whether Accessibility permission is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayPasteDisposition {
    Paste,
    CopyTargetUnavailable,
    CopyFrontmostUnavailable,
    CopyTargetMismatch,
    CopyAccessibilityDenied,
}

/// Decide whether to paste or refuse the synthetic Cmd+V.
///
/// The overlay **canvas** is never a Cmd+V sink — that guard lives in Swift
/// (`insertCaretInCodescribeProbe` → `defer_text_from_overlay`). This function
/// decides among legal ambulances:
/// - Agent window (localized name `Codescribe`) is a legal sink.
/// - Alacritty / Zellij / Notes / … must match, or activation must have
///   confirmed the latched target (the floating overlay can leave
///   `NSWorkspace` still reporting Codescribe).
/// - A third app in front without activation → mismatch, park Paste Here.
pub(super) fn overlay_paste_disposition(
    target_app: Option<&str>,
    frontmost_app: Option<&str>,
    can_post_events: bool,
    activation_confirmed: bool,
) -> OverlayPasteDisposition {
    let Some(target) = target_app.map(str::trim).filter(|name| !name.is_empty()) else {
        return OverlayPasteDisposition::CopyTargetUnavailable;
    };
    if !can_post_events {
        return OverlayPasteDisposition::CopyAccessibilityDenied;
    }
    if target_is_self_app(target) {
        return OverlayPasteDisposition::Paste;
    }
    let Some(frontmost) = frontmost_app.map(str::trim).filter(|name| !name.is_empty()) else {
        return if activation_confirmed {
            OverlayPasteDisposition::Paste
        } else {
            OverlayPasteDisposition::CopyFrontmostUnavailable
        };
    };
    if frontmost.eq_ignore_ascii_case(target) {
        return OverlayPasteDisposition::Paste;
    }
    if target_is_self_app(frontmost) && activation_confirmed {
        return OverlayPasteDisposition::Paste;
    }
    OverlayPasteDisposition::CopyTargetMismatch
}

/// Whether the target was positively observed as frontmost after activation.
/// An accepted activation request is not proof that focus moved; Codescribe
/// remaining frontmost must fail closed into Paste Here.
pub(super) fn overlay_float_still_confirms_activation(
    wait_confirmed: bool,
    _frontmost_after_activate: Option<&str>,
) -> bool {
    wait_confirmed
}

/// Activate the latched ambulance and confirm it owns focus.
///
/// Codescribe is already this process — no activate, and upstream latch policy
/// normally refuses it. A foreign app must activate and match `NSWorkspace`
/// within the bounded wait.
pub(super) fn confirm_latched_paste_target(target_app: Option<&str>) -> bool {
    let Some(name) = target_app.map(str::trim).filter(|n| !n.is_empty()) else {
        return false;
    };
    if target_is_self_app(name) {
        return true;
    }
    if !crate::os::selection::activate_app_by_name(name) {
        return false;
    }
    let waited = crate::os::selection::wait_for_frontmost_app(name, OVERLAY_PASTE_FOCUS_BUDGET);
    overlay_float_still_confirms_activation(
        waited,
        crate::os::selection::current_frontmost_app_name().as_deref(),
    )
}

/// Park a refused synthetic paste in the process-local Paste Here slot.
///
/// Never writes `NSPasteboard`. The user's existing clipboard stays put until
/// they press the Paste Here chord, which then does snapshot → Cmd+V → restore.
pub(super) fn park_refused_paste(payload: String) -> bool {
    crate::os::clipboard::arm_deferred_insert(payload)
}
