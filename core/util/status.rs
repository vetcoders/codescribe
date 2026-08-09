//! Process-wide coarse status signalling.
//!
//! Core code raises a status without knowing who renders it; whoever owns the
//! visible surface (tray, bridge host) installs one callback at startup. The
//! channel is set-once by design, so a late installer cannot silently steal
//! the surface from the one that booted first.

use std::sync::{Arc, OnceLock};

/// Coarse activity state worth showing to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSignal {
    /// Work is in flight — show a busy affordance.
    Thinking,
    /// The last operation failed — show an error affordance.
    Error,
}

/// Renderer-side handler for [`StatusSignal`], shared across threads.
pub type StatusCallback = Arc<dyn Fn(StatusSignal) + Send + Sync>;

static STATUS_CALLBACK: OnceLock<StatusCallback> = OnceLock::new();

/// Install the process-wide status callback. The first caller wins; later
/// calls are dropped rather than replacing an already-live surface.
pub fn set_status_callback(callback: StatusCallback) {
    let _ = STATUS_CALLBACK.set(callback);
}

/// Raise a status signal. A no-op when no callback is installed (headless
/// binaries, tests), so callers never have to check first.
pub fn notify_status(status: StatusSignal) {
    if let Some(callback) = STATUS_CALLBACK.get() {
        (callback)(status);
    }
}
