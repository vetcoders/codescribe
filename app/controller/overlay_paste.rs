//! Overlay Insert / Paste Here delivery disposition and deferred-insert arming.

use std::time::Duration;

use crate::config::DeferredInsertShortcut;

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
/// Pasting requires that focus actually returned to the recorded target: both app
/// names must be present, the frontmost app must not be Codescribe itself, and it
/// must match the target. Only then does missing Accessibility permission become
/// the deciding factor. Any mismatch parks Paste Here (⌘⌥V) and leaves the
/// user's pasteboard alone — it does not fire Cmd+V at whatever window is in
/// front, and it does not overwrite the clipboard the user already had.
pub(super) fn overlay_paste_disposition(
    target_app: Option<&str>,
    frontmost_app: Option<&str>,
    can_post_events: bool,
) -> OverlayPasteDisposition {
    let Some(target) = target_app.map(str::trim).filter(|name| !name.is_empty()) else {
        return OverlayPasteDisposition::CopyTargetUnavailable;
    };
    let Some(frontmost) = frontmost_app.map(str::trim).filter(|name| !name.is_empty()) else {
        return OverlayPasteDisposition::CopyFrontmostUnavailable;
    };
    if frontmost.eq_ignore_ascii_case("codescribe") {
        return OverlayPasteDisposition::CopyTargetMismatch;
    }
    if !frontmost.eq_ignore_ascii_case(target) {
        return OverlayPasteDisposition::CopyTargetMismatch;
    }
    if !can_post_events {
        return OverlayPasteDisposition::CopyAccessibilityDenied;
    }
    OverlayPasteDisposition::Paste
}

/// Park a refused synthetic paste in the process-local Paste Here slot.
///
/// Never writes `NSPasteboard`. The user's existing clipboard stays put until
/// they press the Paste Here chord, which then does snapshot → Cmd+V → restore.
pub(super) fn park_refused_paste(payload: String) -> bool {
    crate::os::clipboard::arm_deferred_insert(payload)
}
