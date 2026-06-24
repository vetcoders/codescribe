//! Overlay window geometry and zoom behavior.
//!
//! Keyability of the borderless window, screen-frame clamping, content max
//! size enforcement, post-resize settling, window delegate callbacks and the
//! Cmd +/-/0 chat zoom with debounced settings save.

use super::*;

static RESIZE_SETTLE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn overlay_window_allows_key_input() -> bool {
    true
}

pub fn overlay_window_allows_main_status() -> bool {
    true
}

pub extern "C" fn can_become_key_window(_this: &Object, _cmd: Sel) -> bool {
    overlay_window_allows_key_input()
}

pub extern "C" fn can_become_main_window(_this: &Object, _cmd: Sel) -> bool {
    overlay_window_allows_main_status()
}

pub extern "C" fn constrain_frame_rect_to_screen(
    this: &Object,
    _cmd: Sel,
    frame_rect: ObjcCGRect,
    screen: Id,
) -> ObjcCGRect {
    unsafe {
        let superclass = Class::get("NSWindow").unwrap();
        let super_rect: ObjcCGRect =
            msg_send![super(this, superclass), constrainFrameRect: frame_rect toScreen: screen];
        let constrained: CGRect = super_rect.into();
        let Some(visible_frame) = visible_frame_for_screen(screen) else {
            return super_rect;
        };

        let (x, y) = clamp_overlay_position(
            visible_frame,
            constrained.size.width,
            constrained.size.height,
            0.0,
            constrained.origin.x,
            constrained.origin.y,
        );

        if (x - constrained.origin.x).abs() <= 0.5 && (y - constrained.origin.y).abs() <= 0.5 {
            super_rect
        } else {
            ObjcCGRect::from(CGRect::new(&CGPoint::new(x, y), &constrained.size))
        }
    }
}

pub extern "C" fn perform_key_equivalent(_this: &Object, _cmd: Sel, event: Id) -> bool {
    unsafe {
        let flags: u64 = msg_send![event, modifierFlags];
        let has_cmd = (flags & (1 << 20)) != 0; // NSEventModifierFlagCommand
        if !has_cmd {
            return false;
        }

        let chars: Id = msg_send![event, charactersIgnoringModifiers];
        if chars.is_null() {
            return false;
        }
        let c_str: *const i8 = msg_send![chars, UTF8String];
        if c_str.is_null() {
            return false;
        }
        let key = std::ffi::CStr::from_ptr(c_str).to_string_lossy();

        match key.as_ref() {
            "=" | "+" => {
                adjust_chat_zoom(0.125);
                true
            }
            "-" => {
                adjust_chat_zoom(-0.125);
                true
            }
            "0" => {
                set_chat_zoom(1.0);
                true
            }
            _ => false,
        }
    }
}

/// Monotonic generation counter for zoom save debounce.
static ZOOM_SAVE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn canonical_zoom_level(zoom: f64) -> f64 {
    UserSettings::normalized_chat_zoom(zoom).unwrap_or(1.0)
}

fn adjust_chat_zoom(delta: f64) {
    let zoom = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let prev = canonical_zoom_level(state.zoom_level);
        let next = canonical_zoom_level(state.zoom_level + delta);
        if (next - prev).abs() < f64::EPSILON {
            None
        } else {
            state.zoom_level = next;
            Some(next)
        }
    };
    let Some(zoom) = zoom else {
        return;
    };
    reflow_agent_after_resize_impl();
    schedule_zoom_save(zoom);
}

fn set_chat_zoom(level: f64) {
    let zoom = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let next = canonical_zoom_level(level);
        if (state.zoom_level - next).abs() < f64::EPSILON {
            None
        } else {
            state.zoom_level = next;
            Some(next)
        }
    };
    let Some(zoom) = zoom else {
        return;
    };
    reflow_agent_after_resize_impl();
    schedule_zoom_save(zoom);
}

/// Debounced save: waits 500ms, then saves only if no newer zoom change occurred.
fn schedule_zoom_save(zoom: f64) {
    let generation = ZOOM_SAVE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if ZOOM_SAVE_GEN.load(std::sync::atomic::Ordering::Relaxed) != generation {
            return; // newer zoom change supersedes
        }
        let mut settings = UserSettings::load();
        if !settings.set_chat_zoom(zoom) {
            debug!("Chat zoom unchanged after debounce; skipping settings save");
        }
    });
}
pub extern "C" fn on_window_will_close(_this: &Object, _cmd: Sel, _notification: Id) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    clear_overlay_state(&mut state);
    debug!("Voice chat overlay closed by user");
}

fn window_visible_frame(window: Id) -> Option<CGRect> {
    unsafe {
        let screen: Id = msg_send![window, screen];
        visible_frame_for_screen(screen)
    }
}

fn visible_frame_for_screen(screen: Id) -> Option<CGRect> {
    unsafe {
        let ns_screen = Class::get("NSScreen").unwrap();
        let mut target_screen = screen;
        if target_screen.is_null() {
            target_screen = msg_send![ns_screen, mainScreen];
        }
        if target_screen.is_null() {
            None
        } else {
            Some(msg_send![target_screen, visibleFrame])
        }
    }
}

fn update_overlay_content_max_size(window: Id) -> Option<CGSize> {
    let visible = window_visible_frame(window)?;
    let max_size = CGSize::new(visible.size.width.min(1000.0), visible.size.height);
    unsafe {
        let _: () = msg_send![window, setContentMaxSize: max_size];
    }
    Some(max_size)
}

fn enforce_overlay_content_max_size(window: Id, animate: bool) {
    let Some(max_size) = update_overlay_content_max_size(window) else {
        return;
    };

    let frame: CGRect = unsafe { msg_send![window, frame] };
    let mut new_frame = frame;
    let mut changed = false;

    if frame.size.width > max_size.width {
        new_frame.size.width = max_size.width;
        changed = true;
    }

    if frame.size.height > max_size.height {
        // Keep top edge visually stable while shrinking height.
        new_frame.origin.y += frame.size.height - max_size.height;
        new_frame.size.height = max_size.height;
        changed = true;
    }

    if changed {
        unsafe {
            let _: () = msg_send![window, setFrame: new_frame display: true animate: animate];
        }
    }
}

fn clamp_overlay_window_to_visible(window: Id) {
    let Some(visible_frame) = window_visible_frame(window) else {
        return;
    };
    let frame: CGRect = unsafe { msg_send![window, frame] };
    // Keep native snap/tile edge alignment; only guarantee visibility.
    let margin = 0.0;

    let (x, y) = clamp_overlay_position(
        visible_frame,
        frame.size.width,
        frame.size.height,
        margin,
        frame.origin.x,
        frame.origin.y,
    );

    if (x - frame.origin.x).abs() > 0.5 || (y - frame.origin.y).abs() > 0.5 {
        unsafe {
            let _: () = msg_send![window, setFrameOrigin: CGPoint::new(x, y)];
        }
    }
}

fn schedule_post_resize_settle(window: Id) {
    let window_ptr = window as usize;
    let generation = RESIZE_SETTLE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(120));
        if RESIZE_SETTLE_GEN.load(std::sync::atomic::Ordering::Relaxed) != generation {
            return;
        }
        Queue::main().exec_async(move || {
            let active_window = {
                let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.window
            };
            if active_window != Some(window_ptr) {
                return;
            }
            let window = window_ptr as Id;
            enforce_overlay_content_max_size(window, false);
            clamp_overlay_window_to_visible(window);
            reflow_overlay_after_resize_impl();
            reflow_agent_after_resize_impl();
        });
    });
}

pub extern "C" fn on_window_did_end_live_resize(_this: &Object, _cmd: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if !window.is_null() {
            enforce_overlay_content_max_size(window, true);
            clamp_overlay_window_to_visible(window);
        }

        // Reflow footer/input and bubbles after resize settles.
        Queue::main().exec_async(|| {
            reflow_overlay_after_resize_impl();
            reflow_agent_after_resize_impl();
        });
    }
}

pub extern "C" fn on_window_did_resize(_this: &Object, _cmd: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if window.is_null() {
            return;
        }
        let in_live_resize: bool = msg_send![window, inLiveResize];
        if in_live_resize {
            return;
        }
        schedule_post_resize_settle(window);
    }
}

pub extern "C" fn on_window_did_change_screen(_this: &Object, _cmd: Sel, notification: Id) {
    unsafe {
        let window: Id = msg_send![notification, object];
        if window.is_null() {
            return;
        }
        enforce_overlay_content_max_size(window, false);
        clamp_overlay_window_to_visible(window);
    }
    Queue::main().exec_async(|| {
        reflow_overlay_after_resize_impl();
        reflow_agent_after_resize_impl();
    });
}
