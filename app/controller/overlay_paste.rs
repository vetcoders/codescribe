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
    /// Focus never left Codescribe; tagged text was copied instead of pasted.
    CopiedToClipboard,
    /// Synthetic event posting is not trusted; tagged text was copied instead.
    AccessibilityPermissionNeeded,
    /// Tagged text is armed in process memory for the global Paste Here command.
    DeferredInsertArmed,
    /// Nothing to deliver (empty transcript).
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPasteResult {
    pub delivery: OverlayPasteDelivery,
    pub target_app_name: Option<String>,
    pub frontmost_app_name: Option<String>,
    pub deferred_insert_shortcut: Option<String>,
    pub deferred_insert_failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeferredInsertRegistration {
    Available { shortcut_label: String },
    Unavailable { reason: String },
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverlayPasteDisposition {
    Paste,
    CopyTargetUnavailable,
    CopyFrontmostUnavailable,
    CopyTargetMismatch,
    CopyAccessibilityDenied,
}

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
