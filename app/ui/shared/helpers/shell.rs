use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSWindowCollectionBehavior, NSWindowStyleMask};

use super::{
    Id, NS_FLOATING_WINDOW_LEVEL, NS_NORMAL_WINDOW_LEVEL, clamp_overlay_position, color_clear,
};

/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_content_view(window: Id) -> Id {
    unsafe { msg_send![window, contentView] }
}

/// Add subview to a view
/// # Safety
/// `parent` and `child` must be valid Objective-C views.
pub unsafe fn add_subview(parent: Id, child: Id) {
    unsafe {
        let _: () = msg_send![parent, addSubview: child];
    }
}

/// Show window (order front)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_show(window: Id) {
    unsafe {
        let _: () = msg_send![window, orderFrontRegardless];
    }
}

/// Present a first-class CodeScribe panel like a normal AppKit window.
///
/// Unlike overlay-only `orderFrontRegardless`, this makes the window key and
/// activates the app so text fields, scroll views, and standard controls behave
/// like Settings/Onboarding.
///
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn present_shared_shell_panel(window: Id) {
    // SAFETY: per the function contract, `window` is a valid `NSWindow`
    // instance. `NSApplication.sharedApplication` returns a singleton retained
    // by the runtime. Caller MUST be on the main thread; `msg_send!` is only
    // valid for AppKit objects from the main thread.
    unsafe {
        if let Some(ns_app) = Class::get("NSApplication") {
            let shared_app: Id = msg_send![ns_app, sharedApplication];
            if !shared_app.is_null() {
                let _: () = msg_send![shared_app, activateIgnoringOtherApps: true];
            }
        }
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = msg_send![window, makeKeyAndOrderFront: nil];
        let _: () = msg_send![window, makeMainWindow];
        let _: () = msg_send![window, orderFrontRegardless];
    }
}

/// Hide window (order out)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_hide(window: Id) {
    unsafe {
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = msg_send![window, orderOut: nil];
    }
}

/// Close window
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_close(window: Id) {
    unsafe {
        let _: () = msg_send![window, close];
    }
}

/// Release an Objective-C object retained via `alloc`/`new`/`copy`/explicit
/// `retain`. Null-safe: no-op if `object` is null.
///
/// # Safety
/// `object` must be a valid Objective-C object pointer or null. Caller must
/// hold a +1 retain that this call balances. After this call, `object`
/// becomes a dangling pointer; do not reuse.
pub unsafe fn release_object(object: Id) {
    if object.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![object, release];
    }
}

/// Close and release a window that should not remain alive after closing.
/// Null-safe: no-op if `window` is null.
///
/// Used for windows created with shared shell policy
/// (`released_when_closed = false`), where `[window close]` does NOT balance
/// the initial `alloc`/`init` retain — manual `release` is required to
/// prevent leaking the `NSWindow` instance.
///
/// IMPORTANT: this helper closes the window FIRST, then releases. AppKit
/// dispatches `windowWillClose` delegate callbacks during `close`, so any
/// delegate or action handler retain that participates in those callbacks
/// MUST be released after this call returns, not before — otherwise the
/// callback chain runs on freed pointers.
///
/// # Safety
/// `window` must be a valid `NSWindow` instance or null. Caller must hold
/// the +1 retain from window construction. After this call, `window`
/// becomes a dangling pointer; do not reuse.
pub unsafe fn window_discard(window: Id) {
    if window.is_null() {
        return;
    }
    unsafe {
        window_close(window);
        release_object(window);
    }
}

/// Set window alpha (for fade animations)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_set_alpha(window: Id, alpha: f64) {
    unsafe {
        let _: () = msg_send![window, setAlphaValue: alpha];
    }
}

/// Shared AppKit shell policy for first-class CodeScribe panels.
///
/// The intent is to keep chat, Settings, and Onboarding in one explicit
/// window-policy matrix while callers continue to own their content trees.
pub struct SharedShellPanelPolicy {
    pub style_mask: NSWindowStyleMask,
    pub backing_store: NSBackingStoreType,
    pub collection_behavior: NSWindowCollectionBehavior,
    pub level: i64,
    pub min_content_size: Option<CGSize>,
    pub max_content_size: Option<CGSize>,
    pub hides_title: bool,
    pub transparent_titlebar: bool,
    pub movable_by_window_background: bool,
    pub opaque: bool,
    pub released_when_closed: bool,
}

/// Visible frame for the main screen, if AppKit can provide one.
pub fn main_screen_visible_frame() -> Option<CGRect> {
    // SAFETY: `Class::get("NSScreen")` returns `None` if the runtime class is
    // not registered (e.g. headless test). When present, `+[NSScreen mainScreen]`
    // is a documented Foundation API returning either nil or a singleton owned
    // by AppKit. Must be called from the main thread; this helper is invoked
    // exclusively by AppKit-side code paths that already hold the main thread.
    unsafe {
        let ns_screen = Class::get("NSScreen")?;
        let screen: Id = msg_send![ns_screen, mainScreen];
        if screen.is_null() {
            None
        } else {
            Some(msg_send![screen, visibleFrame])
        }
    }
}

/// Shared policy for the Agent chat shell.
///
/// **Multi-Space visibility, FLOATING window level.** Operator's directive
/// 2026-05-13: chat overlay must be reachable from any Space and not pin
/// itself to the Space where it was last shown. Prior `FullScreenNone`
/// caused that pinning bug.
///
/// The earlier floating attempt (commit `dc0f9ee`) regressed text input because
/// AppKit would not make the floating overlay keyable. The voice chat window is
/// now allocated through `VoiceChatOverlayWindow`, an `NSWindow` subclass that
/// overrides `canBecomeKeyWindow` and `canBecomeMainWindow`, so the overlay can
/// stay truly always-on-top without dropping Agent input keystrokes.
///
/// `CanJoinAllSpaces` — window follows Space switches.
/// `FullScreenAuxiliary` — window draws over fullscreen apps instead of
///   being banished to the desktop (this behavior is independent of level).
pub fn agent_chat_shell_panel_policy(visible_frame: CGRect) -> SharedShellPanelPolicy {
    SharedShellPanelPolicy {
        style_mask: NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::FullSizeContentView
            | NSWindowStyleMask::Resizable,
        backing_store: NSBackingStoreType::Buffered,
        collection_behavior: NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
        level: NS_FLOATING_WINDOW_LEVEL,
        min_content_size: Some(CGSize::new(380.0, 360.0)),
        max_content_size: Some(CGSize::new(
            visible_frame.size.width.min(1000.0),
            visible_frame.size.height,
        )),
        hides_title: true,
        transparent_titlebar: true,
        movable_by_window_background: true,
        opaque: false,
        released_when_closed: false,
    }
}

/// Shared policy for the native Settings preferences shell.
pub fn settings_shell_panel_policy(fixed_size: CGSize) -> SharedShellPanelPolicy {
    SharedShellPanelPolicy {
        style_mask: NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::FullSizeContentView,
        backing_store: NSBackingStoreType::Buffered,
        collection_behavior: NSWindowCollectionBehavior::FullScreenNone,
        level: NS_NORMAL_WINDOW_LEVEL,
        min_content_size: Some(fixed_size),
        max_content_size: Some(fixed_size),
        hides_title: false,
        transparent_titlebar: true,
        movable_by_window_background: false,
        opaque: true,
        released_when_closed: false,
    }
}

/// Frame an Agent chat shell from the persisted/raw position and clamp to screen.
pub fn agent_chat_shell_frame(
    visible_frame: CGRect,
    window_width: f64,
    window_height: f64,
    margin: f64,
    raw_x: f64,
    raw_y: f64,
) -> CGRect {
    let (x, y) = clamp_overlay_position(
        visible_frame,
        window_width,
        window_height,
        margin,
        raw_x,
        raw_y,
    );
    CGRect::new(
        &CGPoint::new(x, y),
        &CGSize::new(window_width, window_height),
    )
}

/// Apply the shared shell policy to an already-allocated `NSWindow`.
///
/// # Safety
/// `window` must be a valid initialized `NSWindow` instance.
pub unsafe fn apply_shared_shell_panel_policy(window: Id, policy: &SharedShellPanelPolicy) {
    // SAFETY: per the function contract, `window` is a valid initialized
    // `NSWindow`. `policy` is a Rust borrow held for the entire call. Each
    // `msg_send!` setter mutates AppKit-internal state; this MUST run on the
    // main thread (AppKit affinity).
    unsafe {
        let title_visibility = if policy.hides_title { 1_isize } else { 0_isize };
        let _: () = msg_send![window, setTitleVisibility: title_visibility];
        let _: () = msg_send![window, setTitlebarAppearsTransparent: policy.transparent_titlebar];
        let _: () = msg_send![
            window,
            setMovableByWindowBackground: policy.movable_by_window_background
        ];
        let _: () = msg_send![window, setOpaque: policy.opaque];
        if !policy.opaque {
            let _: () = msg_send![window, setBackgroundColor: color_clear()];
        }
        let _: () = msg_send![window, setLevel: policy.level];
        let _: () = msg_send![window, setReleasedWhenClosed: policy.released_when_closed];
        if let Some(min_size) = policy.min_content_size {
            let _: () = msg_send![window, setContentMinSize: min_size];
        }
        if let Some(max_size) = policy.max_content_size {
            let _: () = msg_send![window, setContentMaxSize: max_size];
        }
        let _: () = msg_send![window, setCollectionBehavior: policy.collection_behavior];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_object_is_null_safe() {
        // Null pointer should be a no-op (no segfault, no msg_send to nil).
        // Documented contract: `if object.is_null() { return; }`.
        // Test passes by virtue of completing without segfault — no panic
        // means the null guard worked.
        unsafe {
            release_object(std::ptr::null_mut());
        }
    }

    #[test]
    fn window_discard_is_null_safe() {
        // Same null-safety contract — window_discard guards null before
        // window_close + release_object. Critical because teardown call
        // sites may pass a freshly-taken Option<usize>::None as null Id.
        // Reaching the end of the function without segfault = pass.
        unsafe {
            window_discard(std::ptr::null_mut());
        }
    }

    #[test]
    fn clamp_overlay_position_keeps_window_inside_frame() {
        let visible = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(100.0, 100.0));
        let (x, y) = clamp_overlay_position(visible, 60.0, 60.0, 10.0, 1000.0, -1000.0);
        assert_eq!(x, 30.0);
        assert_eq!(y, 10.0);
    }

    #[test]
    fn agent_chat_shell_policy_caps_to_visible_frame() {
        let visible = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(1200.0, 700.0));
        let policy = agent_chat_shell_panel_policy(visible);

        let min_size = policy.min_content_size.expect("min size");
        assert_eq!(min_size.width, 380.0);
        assert_eq!(min_size.height, 360.0);

        let max_size = policy.max_content_size.expect("max size");
        assert_eq!(max_size.width, 1000.0);
        assert_eq!(max_size.height, 700.0);
        assert_eq!(policy.level, NS_FLOATING_WINDOW_LEVEL);
        assert!(
            policy
                .collection_behavior
                .contains(NSWindowCollectionBehavior::CanJoinAllSpaces)
        );
        assert!(
            policy
                .collection_behavior
                .contains(NSWindowCollectionBehavior::FullScreenAuxiliary)
        );
        assert!(policy.hides_title);
        assert!(!policy.opaque);
        assert!(policy.style_mask.contains(NSWindowStyleMask::Titled));
        assert!(policy.style_mask.contains(NSWindowStyleMask::Closable));
    }

    #[test]
    fn settings_shell_policy_is_fixed_native_panel() {
        let fixed = CGSize::new(840.0, 700.0);
        let policy = settings_shell_panel_policy(fixed);

        assert_eq!(policy.level, NS_NORMAL_WINDOW_LEVEL);
        assert_eq!(
            policy.collection_behavior,
            NSWindowCollectionBehavior::FullScreenNone
        );
        let min_size = policy.min_content_size.expect("min size");
        assert_eq!(min_size.width, fixed.width);
        assert_eq!(min_size.height, fixed.height);
        let max_size = policy.max_content_size.expect("max size");
        assert_eq!(max_size.width, fixed.width);
        assert_eq!(max_size.height, fixed.height);
        assert!(policy.opaque);
        assert!(!policy.hides_title);
        assert!(policy.style_mask.contains(NSWindowStyleMask::Titled));
        assert!(
            policy
                .style_mask
                .contains(NSWindowStyleMask::FullSizeContentView)
        );
        assert!(!policy.style_mask.contains(NSWindowStyleMask::Resizable));
    }
}
