//! Surviving home for `TrayStatus` and `update_tray_status`.
//!
//! Relocated out of the legacy AppKit `app/ui/tray` module. The new SwiftUI
//! app owns the menu bar, but the core still owns the status truth. Producers
//! call `update_tray_status`, and the bridge registers a process-local sink that
//! forwards each status to Swift.
//!
//! Only the pure methods of `TrayStatus` are copied here. The icon-rendering
//! methods (`to_icon` / `to_icon_with_glyph`) stay behind in `app/ui/tray`
//! because they depend on `tray_icon::Icon` and `crate::tray::icons::*`, which
//! die with the legacy AppKit tray.

use std::sync::{Arc, OnceLock, RwLock};

use tracing::trace;

type TrayStatusSink = Arc<dyn Fn(TrayStatusSnapshot) + Send + Sync + 'static>;

/// Status of the Codescribe system, formerly reflected in the AppKit tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    /// App process is visible, but runtime initialization is still in progress.
    Starting,
    /// Idle, waiting for activation
    Idle,
    /// Actively listening/recording
    Listening,
    /// Processing/transcribing
    Thinking,
    /// Successfully completed
    Success,
    /// Error state - backend not available
    Error,
    /// System thermal pressure is high enough to throttle STT.
    Thermal,
    /// A hotkey gesture was detected but blocked before dispatch.
    HotkeyConflict,
}

impl TrayStatus {
    /// Get the human-readable tooltip for this status
    pub fn tooltip(&self) -> String {
        match self {
            TrayStatus::Starting => "Codescribe - Starting...".to_string(),
            TrayStatus::Idle => "Codescribe - Ready".to_string(),
            TrayStatus::Listening => "Codescribe - Recording...".to_string(),
            TrayStatus::Thinking => "Codescribe - Processing...".to_string(),
            TrayStatus::Success => "Codescribe - Done!".to_string(),
            TrayStatus::Error => "Codescribe - Backend unavailable!".to_string(),
            TrayStatus::Thermal => "Codescribe - Thermal throttling".to_string(),
            TrayStatus::HotkeyConflict => "Codescribe - Hotkey conflict".to_string(),
        }
    }

    /// Get the status line text for the menu
    pub fn menu_label(&self) -> &'static str {
        match self {
            TrayStatus::Starting => "Status: Starting...",
            TrayStatus::Idle => "Status: Idle",
            TrayStatus::Listening => "Status: Recording...",
            TrayStatus::Thinking => "Status: Processing...",
            TrayStatus::Success => "Status: Done!",
            TrayStatus::Error => "Status: Error",
            TrayStatus::Thermal => "Status: Thermal throttling",
            TrayStatus::HotkeyConflict => "Status: Hotkey conflict",
        }
    }
}

/// Tray status plus session lane. `assistive` is retained even while the visible
/// status is idle/starting so the next Listening/Thinking beat can tint correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayStatusSnapshot {
    pub status: TrayStatus,
    pub assistive: bool,
}

impl TrayStatusSnapshot {
    pub fn new(status: TrayStatus, assistive: bool) -> Self {
        Self { status, assistive }
    }

    pub fn is_assistive_visible(&self) -> bool {
        self.assistive && matches!(self.status, TrayStatus::Listening | TrayStatus::Thinking)
    }

    pub fn tooltip(&self) -> String {
        if self.is_assistive_visible() {
            match self.status {
                TrayStatus::Listening => "Codescribe - Agent listening...".to_string(),
                TrayStatus::Thinking => "Codescribe - Agent processing...".to_string(),
                _ => self.status.tooltip(),
            }
        } else {
            self.status.tooltip()
        }
    }

    pub fn menu_label(&self) -> &'static str {
        if self.is_assistive_visible() {
            match self.status {
                TrayStatus::Listening => "Status: Agent listening...",
                TrayStatus::Thinking => "Status: Agent processing...",
                _ => self.status.menu_label(),
            }
        } else {
            self.status.menu_label()
        }
    }
}

fn current_status_store() -> &'static RwLock<TrayStatusSnapshot> {
    static CURRENT_STATUS: OnceLock<RwLock<TrayStatusSnapshot>> = OnceLock::new();
    CURRENT_STATUS.get_or_init(|| RwLock::new(TrayStatusSnapshot::new(TrayStatus::Idle, false)))
}

fn tray_status_sink_store() -> &'static RwLock<Option<TrayStatusSink>> {
    static TRAY_STATUS_SINK: OnceLock<RwLock<Option<TrayStatusSink>>> = OnceLock::new();
    TRAY_STATUS_SINK.get_or_init(|| RwLock::new(None))
}

/// Register the process-local bridge sink that mirrors core status to Swift.
///
/// The app crate cannot depend on the UniFFI bridge (the bridge wraps this
/// crate), so the bridge injects a plain callback here instead.
pub fn set_tray_status_sink(sink: Option<TrayStatusSink>) {
    let mut guard = tray_status_sink_store()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    *guard = sink;
}

/// Latest core-side tray status, used to seed new Swift listeners.
pub fn current_tray_status() -> TrayStatus {
    current_status_store()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .status
}

/// Latest core-side tray status snapshot, including the active assistive lane.
pub fn current_tray_status_snapshot() -> TrayStatusSnapshot {
    *current_status_store()
        .read()
        .unwrap_or_else(|error| error.into_inner())
}

/// Update only the active assistive lane and notify Swift if the payload changed.
pub fn set_tray_assistive_session(assistive: bool) {
    let snapshot = {
        let mut current = current_status_store()
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let next = TrayStatusSnapshot::new(current.status, assistive);
        if *current == next {
            return;
        }
        *current = next;
        next
    };

    notify_tray_status(snapshot);
}

/// Update the menu-bar status truth and notify the Swift bridge when registered.
///
/// Kept callable with the same signature surviving producers expect, but no
/// longer a stub: hotkeys, thermal throttling, and recording lifecycle changes
/// flow through the registered bridge sink.
pub fn update_tray_status(status: TrayStatus) {
    let snapshot = {
        let mut current = current_status_store()
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let next = TrayStatusSnapshot::new(status, current.assistive);
        *current = next;
        next
    };

    notify_tray_status(snapshot);
}

fn notify_tray_status(snapshot: TrayStatusSnapshot) {
    let sink = tray_status_sink_store()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(Arc::clone);

    if let Some(sink) = sink {
        sink(snapshot);
    } else {
        trace!(
            status = ?snapshot.status,
            assistive = snapshot.assistive,
            "tray status updated before Swift bridge listener registration"
        );
    }
}
