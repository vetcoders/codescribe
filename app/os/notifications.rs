//! macOS user notifications via UNUserNotificationCenter.
//!
//! Uses the modern UserNotifications framework (available since macOS 10.14).
//! Requires a proper app bundle — bare binaries (e.g. `~/.cargo/bin/`) get a
//! graceful no-op with a tracing::warn instead of an ObjC exception crash.
//!
//! Vibecrafted with AI Agents by VetCoders (c)2026 VetCoders

use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use std::ffi::CString;
use std::sync::Once;
use tracing::warn;

// Ensure UserNotifications.framework is linked
#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

type Id = *mut Object;

static AUTH_ONCE: Once = Once::new();

fn ns_string(s: &str) -> Id {
    let sanitized = if s.as_bytes().contains(&0) {
        warn!("Notification text contained NUL bytes; stripping before NSString conversion");
        s.chars().filter(|&ch| ch != '\0').collect::<String>()
    } else {
        s.to_string()
    };

    unsafe {
        let Some(cls) = Class::get("NSString") else {
            warn!("Notification skipped: NSString class unavailable");
            return std::ptr::null_mut();
        };
        let Ok(c_str) = CString::new(sanitized) else {
            warn!("Notification skipped: failed to convert text into CString");
            return std::ptr::null_mut();
        };
        msg_send![cls, stringWithUTF8String: c_str.as_ptr()]
    }
}

fn notification_identifier() -> Id {
    ns_string(&format!(
        "codescribe-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ))
}

/// Check if we're running inside a proper app bundle.
/// UNUserNotificationCenter crashes with NSInternalInconsistencyException
/// ("bundleProxyForCurrentProcess is nil") when called from a bare binary.
fn has_app_bundle() -> bool {
    unsafe {
        let cls = match Class::get("NSBundle") {
            Some(c) => c,
            None => return false,
        };
        let bundle: Id = msg_send![cls, mainBundle];
        if bundle.is_null() {
            return false;
        }
        let identifier: Id = msg_send![bundle, bundleIdentifier];
        !identifier.is_null()
    }
}

/// Request notification authorization (once per app lifetime).
/// Non-blocking — fires callback internally, we ignore the result.
fn ensure_authorized() {
    AUTH_ONCE.call_once(|| {
        unsafe {
            let center = notification_center();
            if center.is_null() {
                return;
            }
            // UNAuthorizationOptionAlert | UNAuthorizationOptionSound = (1<<2) | (1<<1) = 6
            let options: usize = 6;
            // Pass nil block — we don't care about the result for local notifications
            let nil_block: Id = std::ptr::null_mut();
            let _: () =
                msg_send![center, requestAuthorizationWithOptions:options completionHandler:nil_block];
        }
    });
}

fn notification_center() -> Id {
    unsafe {
        if !has_app_bundle() {
            return std::ptr::null_mut();
        }
        let cls = match Class::get("UNUserNotificationCenter") {
            Some(c) => c,
            None => return std::ptr::null_mut(),
        };
        msg_send![cls, currentNotificationCenter]
    }
}

/// Show a local notification with title and body.
///
/// Best-effort: silently does nothing if UserNotifications framework
/// is unavailable or authorization was denied.
pub fn notify(title: &str, body: &str) {
    ensure_authorized();

    unsafe {
        let center = notification_center();
        if center.is_null() {
            warn!("Notification skipped (no app bundle): {title} — {body}");
            return;
        }

        // UNMutableNotificationContent
        let content_cls = match Class::get("UNMutableNotificationContent") {
            Some(c) => c,
            None => return,
        };
        let content: Id = msg_send![content_cls, new];
        if content.is_null() {
            return;
        }

        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setBody: ns_string(body)];

        // Default sound
        let sound_cls = match Class::get("UNNotificationSound") {
            Some(c) => c,
            None => return,
        };
        let sound: Id = msg_send![sound_cls, defaultSound];
        let _: () = msg_send![content, setSound: sound];

        // UNNotificationRequest with nil trigger = deliver immediately
        let request_cls = match Class::get("UNNotificationRequest") {
            Some(c) => c,
            None => return,
        };

        // Unique ID per notification (timestamp-based)
        let identifier = notification_identifier();
        if identifier.is_null() {
            return;
        }
        let trigger: Id = std::ptr::null_mut();
        let request: Id = msg_send![request_cls, requestWithIdentifier:identifier content:content trigger:trigger];

        // nil completion handler
        let nil_block: Id = std::ptr::null_mut();
        let _: () =
            msg_send![center, addNotificationRequest:request withCompletionHandler:nil_block];
    }
}

/// Show notification with title, subtitle, and body.
pub fn notify_with_subtitle(title: &str, subtitle: &str, body: &str) {
    ensure_authorized();

    unsafe {
        let center = notification_center();
        if center.is_null() {
            warn!("Notification skipped (no app bundle): {title} — {subtitle} — {body}");
            return;
        }

        let content_cls = match Class::get("UNMutableNotificationContent") {
            Some(c) => c,
            None => return,
        };
        let content: Id = msg_send![content_cls, new];
        if content.is_null() {
            return;
        }

        let _: () = msg_send![content, setTitle: ns_string(title)];
        let _: () = msg_send![content, setSubtitle: ns_string(subtitle)];
        let _: () = msg_send![content, setBody: ns_string(body)];

        let sound_cls = match Class::get("UNNotificationSound") {
            Some(c) => c,
            None => return,
        };
        let sound: Id = msg_send![sound_cls, defaultSound];
        let _: () = msg_send![content, setSound: sound];

        let request_cls = match Class::get("UNNotificationRequest") {
            Some(c) => c,
            None => return,
        };
        let identifier = notification_identifier();
        if identifier.is_null() {
            return;
        }
        let trigger: Id = std::ptr::null_mut();
        let request: Id = msg_send![request_cls, requestWithIdentifier:identifier content:content trigger:trigger];

        let nil_block: Id = std::ptr::null_mut();
        let _: () =
            msg_send![center, addNotificationRequest:request withCompletionHandler:nil_block];
    }
}
