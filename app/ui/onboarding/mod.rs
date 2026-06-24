//! First-run onboarding wizard.
//!
//! Module layout (decomposed from a single 2259-LOC file):
//! - [`steps`] — step metadata: permission kinds, recovery strategies, flow order
//! - [`state`] — wizard state, UI element refs, initial-state probes
//! - [`session`] — on-disk markers, resume progress, flock(2) session lock
//! - [`permission_flow`] — TCC status mapping, requests, runtime reconciliation
//! - [`handlers`] — Objective-C action handler / window delegate bridge
//! - [`window`] — NSWindow construction and static UI build
//! - [`render`] — per-step rendering of the built UI
//! - [`actions`] — flow control, choice persistence, finish/teardown
//! - [`widgets`] — onboarding-local AppKit widget glue
//!
//! External contract: `should_show_onboarding` / `show_onboarding_wizard`
//! (re-exported from `app/lib.rs`), and the permission surface shared with
//! Settings (`PermissionKind`, `PERMISSION_ORDER`, `permission_status`,
//! `request_permission`, `open_permission_settings`,
//! `reconcile_permission_runtime_after_grant`).

mod actions;
mod handlers;
mod permission_flow;
mod render;
mod session;
mod state;
mod steps;
#[cfg(test)]
mod tests;
mod widgets;
mod window;

use dispatch::Queue;

pub(crate) use self::permission_flow::{
    PERMISSION_ORDER, open_permission_settings, permission_status,
    reconcile_permission_runtime_after_grant, request_permission,
};
pub use self::session::should_show_onboarding;
pub(crate) use self::steps::PermissionKind;

// Type alias for Objective-C object pointers
pub use crate::ui_helpers::Id;

pub fn show_onboarding_wizard() {
    if !should_show_onboarding() {
        return;
    }
    if !session::acquire_onboarding_lock() {
        return;
    }

    if window::is_main_thread() {
        window::launch_onboarding_window();
    } else {
        Queue::main().exec_async(move || {
            window::launch_onboarding_window();
        });
    }
}
