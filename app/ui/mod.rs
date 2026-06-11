pub mod onboarding;
pub mod overlay;
pub mod settings;
pub mod shared;
pub mod tray;
pub mod voice_chat;
// macOS UI utilities for hold badge indicator and caret tracing
// - Displaying a floating red badge indicator during recording
// - Tracking text caret position via Accessibility API
// - Falling back to cursor position when caret is unavailable

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::Class;
use objc::runtime::Sel;
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::ptr;

use crate::ui::shared::helpers::{add_subview, window_close, window_show};
use crate::ui_helpers::ns_string;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

// Type alias for Objective-C object pointers (compatible with objc crate msg_send!)
use crate::ui_helpers::Id;

// Accessibility API bindings (use raw pointers compatible with C FFI)
type AXId = *mut std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCopyAttributeValue(element: AXId, attribute: AXId, value: *mut AXId) -> i32;
    fn AXUIElementCreateSystemWide() -> AXId;
    fn AXValueGetValue(value: AXId, type_: i32, value_ptr: *mut std::ffi::c_void) -> bool;
    fn CFRelease(cf: *const std::ffi::c_void);
}

// AX constants
const AX_ERROR_SUCCESS: i32 = 0;
const AX_FOCUSED_UIELEMENT_ATTRIBUTE: &str = "AXFocusedUIElement";
const AX_ROLE_ATTRIBUTE: &str = "AXRole";
const AX_SELECTED_TEXT_ATTRIBUTE: &str = "AXSelectedText";
const AX_SELECTED_TEXT_RANGE_ATTRIBUTE: &str = "AXSelectedTextRange";
const AX_POSITION_ATTRIBUTE: &str = "AXPosition";
const AX_SIZE_ATTRIBUTE: &str = "AXSize";

// AXValue types
const AX_VALUE_CGPOINT_TYPE: i32 = 1;
const AX_VALUE_CGSIZE_TYPE: i32 = 2;
const AX_VALUE_CFRANGE_TYPE: i32 = 3;

// Window level constants
const NS_STATUS_WINDOW_LEVEL: i64 = 25;

/// Badge display mode for different recording/processing states
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BadgeMode {
    /// Hold mode (Ctrl): Red, solid - "trzymaj palec!"
    Hold,
    /// Toggle mode (⌥⌥): Red, pulsing - "nagrywam hands-off"
    Toggle,
    /// Processing: Orange - "transkrybuję/formatuję"
    Processing,
    /// AI mode (Chat/Selection): Purple with glow - "AI słucha"
    Assistive,
}

impl BadgeMode {
    /// Get the base color for this mode (RGBA)
    pub fn color(&self) -> (f64, f64, f64, f64) {
        match self {
            BadgeMode::Hold => (1.0, 0.0, 0.0, 0.8),        // Red
            BadgeMode::Toggle => (1.0, 0.0, 0.0, 0.8),      // Red (will pulse)
            BadgeMode::Processing => (1.0, 0.5, 0.0, 0.85), // Orange
            BadgeMode::Assistive => (0.6, 0.2, 0.9, 0.85),  // Purple
        }
    }

    /// Whether this mode should pulse (opacity animation)
    pub fn should_pulse(&self) -> bool {
        matches!(self, BadgeMode::Toggle | BadgeMode::Processing)
    }

    /// Whether this mode has glow effect
    pub fn has_glow(&self) -> bool {
        matches!(self, BadgeMode::Assistive)
    }

    /// Get diameter multiplier for this mode
    pub fn diameter_multiplier(&self) -> f64 {
        match self {
            BadgeMode::Assistive => 1.2, // Slightly larger for AI mode
            _ => 1.0,
        }
    }
}

/// Configuration for the hold badge
#[derive(Debug, Clone)]
pub struct HoldBadgeConfig {
    /// Diameter of the badge circle in pixels
    pub diameter: f64,
    /// Offset from caret/cursor position (x, y)
    pub offset: (f64, f64),
    /// Update interval in milliseconds
    pub update_interval_ms: u64,
    /// Badge color (R, G, B, A)
    pub color: (f64, f64, f64, f64),
    /// Badge mode for animations
    pub mode: BadgeMode,
}

impl Default for HoldBadgeConfig {
    fn default() -> Self {
        Self {
            diameter: 12.0,
            offset: (10.0, -10.0),
            update_interval_ms: 150,
            color: (1.0, 0.0, 0.0, 0.8), // Red with 80% opacity
            mode: BadgeMode::Hold,
        }
    }
}

impl HoldBadgeConfig {
    /// Create config from badge mode with appropriate colors
    pub fn from_mode(mode: BadgeMode) -> Self {
        let base = Self::default();
        Self {
            diameter: base.diameter * mode.diameter_multiplier(),
            color: mode.color(),
            mode,
            ..base
        }
    }
}

/// Hold badge state
struct HoldBadgeState {
    window: Option<usize>, // Store as usize to make it Send
    timer_running: bool,
    config: HoldBadgeConfig,
    last_position: (f64, f64),
}

lazy_static::lazy_static! {
    static ref BADGE_STATE: Arc<Mutex<HoldBadgeState>> = Arc::new(Mutex::new(HoldBadgeState {
        window: None,
        timer_running: false,
        config: HoldBadgeConfig::default(),
        last_position: (f64::NAN, f64::NAN),
    }));
}

/// Check if the currently focused element accepts text input
pub fn focused_element_accepts_text() -> bool {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return false;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != AX_ERROR_SUCCESS || focused_element.is_null() {
            return false;
        }

        // Get role attribute
        let mut role_value: AXId = ptr::null_mut();
        let role_attr = CFString::new(AX_ROLE_ATTRIBUTE);
        let role_result = AXUIElementCopyAttributeValue(
            focused_element,
            role_attr.as_concrete_TypeRef() as AXId,
            &mut role_value,
        );

        CFRelease(focused_element);

        if role_result != AX_ERROR_SUCCESS || role_value.is_null() {
            return false;
        }

        // Convert role to string
        let role_str = CFString::wrap_under_get_rule(role_value as *const _);
        let role = role_str.to_string();
        CFRelease(role_value);

        // Check if role indicates text input
        matches!(
            role.as_str(),
            "AXTextArea" | "AXTextField" | "AXComboBox" | "AXTextView" | "AXWebArea"
        )
    }
}

/// Get the current text caret position in screen coordinates
pub fn get_caret_position() -> Option<(f64, f64)> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != AX_ERROR_SUCCESS || focused_element.is_null() {
            return None;
        }

        // Get selected text range
        let mut range_value: AXId = ptr::null_mut();
        let range_attr = CFString::new(AX_SELECTED_TEXT_RANGE_ATTRIBUTE);
        let range_result = AXUIElementCopyAttributeValue(
            focused_element,
            range_attr.as_concrete_TypeRef() as AXId,
            &mut range_value,
        );

        if range_result != AX_ERROR_SUCCESS || range_value.is_null() {
            CFRelease(focused_element);
            return None;
        }

        // Extract range
        #[repr(C)]
        struct CFRange {
            location: i64,
            length: i64,
        }

        let mut cf_range = CFRange {
            location: 0,
            length: 0,
        };

        let range_ok = AXValueGetValue(
            range_value,
            AX_VALUE_CFRANGE_TYPE,
            &mut cf_range as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(range_value);

        if !range_ok {
            CFRelease(focused_element);
            return None;
        }

        // Try to get position and size of the focused element
        let mut position_value: AXId = ptr::null_mut();
        let position_attr = CFString::new(AX_POSITION_ATTRIBUTE);
        let position_result = AXUIElementCopyAttributeValue(
            focused_element,
            position_attr.as_concrete_TypeRef() as AXId,
            &mut position_value,
        );

        let mut size_value: AXId = ptr::null_mut();
        let size_attr = CFString::new(AX_SIZE_ATTRIBUTE);
        let size_result = AXUIElementCopyAttributeValue(
            focused_element,
            size_attr.as_concrete_TypeRef() as AXId,
            &mut size_value,
        );

        CFRelease(focused_element);

        if position_result != AX_ERROR_SUCCESS
            || position_value.is_null()
            || size_result != AX_ERROR_SUCCESS
            || size_value.is_null()
        {
            if !position_value.is_null() {
                CFRelease(position_value);
            }
            if !size_value.is_null() {
                CFRelease(size_value);
            }
            return None;
        }

        // Extract position
        let mut position = CGPoint { x: 0.0, y: 0.0 };
        let position_ok = AXValueGetValue(
            position_value,
            AX_VALUE_CGPOINT_TYPE,
            &mut position as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(position_value);

        // Extract size
        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };
        let size_ok = AXValueGetValue(
            size_value,
            AX_VALUE_CGSIZE_TYPE,
            &mut size as *mut _ as *mut std::ffi::c_void,
        );

        CFRelease(size_value);

        if !position_ok || !size_ok {
            return None;
        }

        // Estimate caret position (top-left of element + small offset)
        // For better accuracy, we'd need to parse the text layout, but this is a reasonable approximation
        Some((position.x, position.y + size.height / 2.0))
    }
}

/// Get currently selected text from the focused UI element (best-effort).
///
/// Notes:
/// - Requires Accessibility permission.
/// - Many apps expose selected text via `AXSelectedText`, but not all.
/// - Returns `None` if there's no selection or the attribute isn't supported.
pub fn get_selected_text(max_chars: usize) -> Option<String> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != AX_ERROR_SUCCESS || focused_element.is_null() {
            return None;
        }

        let mut selected_value: AXId = ptr::null_mut();
        let selected_attr = CFString::new(AX_SELECTED_TEXT_ATTRIBUTE);
        let selected_result = AXUIElementCopyAttributeValue(
            focused_element,
            selected_attr.as_concrete_TypeRef() as AXId,
            &mut selected_value,
        );

        CFRelease(focused_element);

        if selected_result != AX_ERROR_SUCCESS || selected_value.is_null() {
            return None;
        }

        let selected_str = CFString::wrap_under_get_rule(selected_value as *const _).to_string();
        CFRelease(selected_value);

        let mut s = selected_str.trim().to_string();
        if s.is_empty() {
            return None;
        }

        let char_count = s.chars().count();
        if max_chars > 0 && char_count > max_chars {
            s = s.chars().take(max_chars).collect();
            s.push('…');
        }

        Some(s)
    }
}

/// Get the current selected text length (range length) from the focused UI element.
///
/// Returns:
/// - `Some(0)` if the element supports `AXSelectedTextRange` but there's no selection
/// - `Some(n>0)` if there's a selection
/// - `None` if the attribute isn't available or any step fails
pub fn get_selected_text_length() -> Option<usize> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let mut focused_element: AXId = ptr::null_mut();
        let attr_name = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
        let result = AXUIElementCopyAttributeValue(
            system_wide,
            attr_name.as_concrete_TypeRef() as AXId,
            &mut focused_element,
        );

        CFRelease(system_wide);

        if result != AX_ERROR_SUCCESS || focused_element.is_null() {
            return None;
        }

        let mut range_value: AXId = ptr::null_mut();
        let range_attr = CFString::new(AX_SELECTED_TEXT_RANGE_ATTRIBUTE);
        let range_result = AXUIElementCopyAttributeValue(
            focused_element,
            range_attr.as_concrete_TypeRef() as AXId,
            &mut range_value,
        );

        CFRelease(focused_element);

        if range_result != AX_ERROR_SUCCESS || range_value.is_null() {
            return None;
        }

        #[repr(C)]
        struct CFRange {
            location: i64,
            length: i64,
        }

        let mut cf_range = CFRange {
            location: 0,
            length: 0,
        };

        let ok = AXValueGetValue(
            range_value,
            AX_VALUE_CFRANGE_TYPE,
            &mut cf_range as *mut _ as *mut std::ffi::c_void,
        );
        CFRelease(range_value);

        if !ok {
            return None;
        }

        Some(cf_range.length.max(0) as usize)
    }
}

/// Get the current mouse cursor position in screen coordinates
pub fn get_cursor_position() -> (f64, f64) {
    let mouse_location = NSEvent::mouseLocation();
    (mouse_location.x, mouse_location.y)
}

/// Get the best available position for the badge (caret or cursor)
fn get_badge_position() -> (f64, f64) {
    get_caret_position().unwrap_or_else(get_cursor_position)
}

/// Create the hold badge window
unsafe fn create_badge_window(config: &HoldBadgeConfig) -> Id {
    let ns_window = Class::get("NSWindow").unwrap();

    // Get initial position
    let (x, y) = get_badge_position();
    let adjusted_x = x + config.offset.0;
    let adjusted_y = y + config.offset.1;
    debug!(
        "Badge position: raw=({:.1}, {:.1}), adjusted=({:.1}, {:.1}), diameter={}",
        x, y, adjusted_x, adjusted_y, config.diameter
    );

    // Create window frame using CGRect (screen coordinates)
    let window_frame = CGRect {
        origin: CGPoint {
            x: adjusted_x,
            y: adjusted_y,
        },
        size: CGSize {
            width: config.diameter,
            height: config.diameter,
        },
    };

    // Create window
    let window: Id = msg_send![ns_window, alloc];
    let style_mask = NSWindowStyleMask::Borderless;
    let backing = NSBackingStoreType::Buffered;
    let window: Id = msg_send![
        window,
        initWithContentRect: window_frame
        styleMask: style_mask
        backing: backing
        defer: false
    ];

    // Configure window for floating transparent overlay
    let clear_color = NSColor::clearColor();
    let clear_color_ptr = &*clear_color as *const _ as Id;
    let _: () = msg_send![window, setOpaque: false];
    let _: () = msg_send![window, setBackgroundColor: clear_color_ptr];
    let _: () = msg_send![window, setIgnoresMouseEvents: true];
    let _: () = msg_send![window, setLevel: NS_STATUS_WINDOW_LEVEL];
    // Ensure any helper windows show up over fullscreen Spaces.
    let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

    // Enable layer-backed views for better transparency/compositing
    let content_view: Id = msg_send![window, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];

    // Create badge view (circular red indicator)
    // SAFETY: create_badge_view is unsafe, called from unsafe fn
    let badge_view = unsafe { create_badge_view(config) };
    unsafe {
        add_subview(content_view, badge_view);
    }

    // Force the view to display
    let _: () = msg_send![badge_view, setNeedsDisplay: true];

    window
}

/// Create the circular badge view using CALayer for reliable rendering
unsafe fn create_badge_view(config: &HoldBadgeConfig) -> Id {
    // Use a plain NSView with a CALayer for drawing
    let ns_view = Class::get("NSView").unwrap();
    let view: Id = msg_send![ns_view, alloc];
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: config.diameter,
            height: config.diameter,
        },
    };
    let view: Id = msg_send![view, initWithFrame: frame];

    // Enable layer-backing
    let _: () = msg_send![view, setWantsLayer: true];

    // Get the layer
    let layer: Id = msg_send![view, layer];
    if layer.is_null() {
        warn!("Badge layer is null - badge will not be visible");
        return view;
    }

    // Configure the layer to draw a circle
    // Set background color from config (default: red with 80% opacity)
    // SAFETY: FFI calls to CoreGraphics
    let cg_color = unsafe {
        create_cg_color(
            config.color.0,
            config.color.1,
            config.color.2,
            config.color.3,
        )
    };
    let _: () = msg_send![layer, setBackgroundColor: cg_color];
    // SAFETY: Releasing CGColor we just created
    unsafe { CGColorRelease(cg_color) };

    // Make it circular by setting corner radius to half the diameter
    let corner_radius = config.diameter / 2.0;
    let _: () = msg_send![layer, setCornerRadius: corner_radius];

    // Ensure the layer clips to bounds (for the circle shape)
    let _: () = msg_send![layer, setMasksToBounds: true];

    view
}

// CGColor functions
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorCreate(
        space: *const std::ffi::c_void,
        components: *const f64,
    ) -> *const std::ffi::c_void;
    fn CGColorSpaceCreateDeviceRGB() -> *const std::ffi::c_void;
    fn CGColorSpaceRelease(space: *const std::ffi::c_void);
    fn CGColorRelease(color: *const std::ffi::c_void);
}

/// Create a CGColor from RGBA components
unsafe fn create_cg_color(r: f64, g: f64, b: f64, a: f64) -> *const std::ffi::c_void {
    unsafe {
        let color_space = CGColorSpaceCreateDeviceRGB();
        let components: [f64; 4] = [r, g, b, a];
        let color = CGColorCreate(color_space, components.as_ptr());
        CGColorSpaceRelease(color_space);
        color
    }
}

/// Show the hold badge and start position tracking (default: Hold mode)
pub fn show_hold_badge() {
    show_hold_badge_with_config(HoldBadgeConfig::default());
}

/// Show badge for specific mode with appropriate color/animation
pub fn show_badge_for_mode(mode: BadgeMode) {
    show_hold_badge_with_config(HoldBadgeConfig::from_mode(mode));
}

/// Internal implementation that must run on the main thread
fn show_hold_badge_impl(config: HoldBadgeConfig) {
    debug!("Showing hold badge (diameter={})", config.diameter);
    unsafe {
        // IMPORTANT: do not hold BADGE_STATE while calling `window_close`.
        // Closing a window can trigger AppKit callbacks/notifications which may
        // re-enter our code and attempt to lock BADGE_STATE again → deadlock.
        let old_window_ptr = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.window.take()
        };
        if let Some(window_ptr) = old_window_ptr {
            window_close(window_ptr as Id);
        }

        // Create new badge window (MUST be on main thread)
        let window = create_badge_window(&config);

        // Make window visible - use orderFrontRegardless which works even when app is not active
        window_show(window);

        // Force content view to redraw
        let content_view: Id = msg_send![window, contentView];
        let _: () = msg_send![content_view, setNeedsDisplay: true];

        // Update shared state and determine whether we need to start the updater thread.
        let (update_interval, start_updater) = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let was_running = state.timer_running;
            state.window = Some(window as usize);
            state.config = config.clone();
            state.timer_running = true;
            (config.update_interval_ms, !was_running)
        };

        // Start a SINGLE position updater thread. Subsequent calls to `show_*` just update state.
        if start_updater {
            thread::spawn(move || {
                let mut pulse_phase: f64 = 0.0;
                let pulse_speed = 0.15; // Radians per update cycle

                loop {
                    thread::sleep(Duration::from_millis(update_interval));

                    // Snapshot state without blocking the main thread. Never hold the lock
                    // during AX queries (get_badge_position), which can be slow or stall.
                    let (window_ptr, config, should_pulse, last_position) = {
                        use std::sync::TryLockError;
                        let state = match BADGE_STATE.try_lock() {
                            Ok(state) => state,
                            Err(TryLockError::Poisoned(err)) => err.into_inner(),
                            Err(TryLockError::WouldBlock) => {
                                // Skip this tick if the state is busy; avoid blocking UI.
                                continue;
                            }
                        };
                        if !state.timer_running {
                            break;
                        }
                        (
                            state.window,
                            state.config.clone(),
                            state.config.mode.should_pulse(),
                            state.last_position,
                        )
                    };

                    let Some(window_ptr) = window_ptr else {
                        continue;
                    };

                    // Calculate pulse opacity (sine wave from 0.4 to 1.0)
                    let pulse_opacity = if should_pulse {
                        pulse_phase += pulse_speed;
                        0.7 + 0.3 * pulse_phase.sin() // Range: 0.4 to 1.0
                    } else {
                        1.0
                    };

                    // Check if cursor actually moved (hysteresis: skip if < 2px)
                    let (new_x, new_y) = get_badge_position();
                    let (lx, ly) = last_position;
                    let position_changed =
                        lx.is_nan() || (new_x - lx).abs() > 2.0 || (new_y - ly).abs() > 2.0;

                    // Skip main-thread dispatch entirely when nothing visual changed
                    if !position_changed && !should_pulse {
                        continue;
                    }

                    let adjusted_x = new_x + config.offset.0;
                    let adjusted_y = new_y + config.offset.1;

                    // Position and opacity updates need main thread
                    Queue::main().exec_async(move || {
                        // Ensure state is still valid; do not block UI if busy.
                        {
                            use std::sync::TryLockError;
                            let mut state = match BADGE_STATE.try_lock() {
                                Ok(state) => state,
                                Err(TryLockError::Poisoned(err)) => err.into_inner(),
                                Err(TryLockError::WouldBlock) => return,
                            };
                            if !state.timer_running || state.window != Some(window_ptr) {
                                return;
                            }
                            state.last_position = (new_x, new_y);
                        }

                        let window = window_ptr as Id;
                        if position_changed {
                            let new_origin = CGPoint {
                                x: adjusted_x,
                                y: adjusted_y,
                            };
                            let _: () = msg_send![window, setFrameOrigin: new_origin];
                        }

                        if should_pulse {
                            let content_view: Id = msg_send![window, contentView];
                            if !content_view.is_null() {
                                let subviews: Id = msg_send![content_view, subviews];
                                let count: usize = msg_send![subviews, count];
                                if count > 0 {
                                    let badge_view: Id = msg_send![subviews, objectAtIndex: 0usize];
                                    let layer: Id = msg_send![badge_view, layer];
                                    if !layer.is_null() {
                                        let _: () =
                                            msg_send![layer, setOpacity: pulse_opacity as f32];
                                    }
                                }
                            }
                        }
                    });
                }
            });
        }
    }
}

/// Show the hold badge with custom configuration
/// This dispatches to the main thread for thread safety with NSWindow
pub fn show_hold_badge_with_config(config: HoldBadgeConfig) {
    // Check if we're already on the main thread by checking thread name
    // Note: exec_sync on main queue from main thread causes deadlock
    let is_main_thread = std::thread::current().name() == Some("main");

    if is_main_thread {
        show_hold_badge_impl(config);
    } else {
        // Dispatch to main thread - NSWindow MUST be created on main thread
        // Using exec_async to avoid deadlock when called from tokio runtime
        Queue::main().exec_async(move || {
            show_hold_badge_impl(config);
        });
    }
}

/// Hide the hold badge and stop position tracking
/// This dispatches to the main thread for thread safety with NSWindow
pub fn hide_hold_badge() {
    debug!("Hiding hold badge");

    // Stop the timer first (can be done on any thread)
    let window_ptr = {
        let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.timer_running = false;
        state.window.take()
    };

    // Dispatch window close to main thread. Do NOT hold BADGE_STATE while closing.
    Queue::main().exec_async(move || {
        if let Some(window_ptr) = window_ptr {
            unsafe {
                window_close(window_ptr as Id);
            }
        }
    });
}

/// Embedded icon for Dock (same as tray icon source)
const DOCK_ICON_BYTES: &[u8] = include_bytes!("../../assets/icon.png");

#[cfg(target_os = "macos")]
const NS_APP_ACTIVATION_POLICY_REGULAR: isize = 0;
#[cfg(target_os = "macos")]
const NS_APP_ACTIVATION_POLICY_ACCESSORY: isize = 1;

#[cfg(target_os = "macos")]
fn dock_activation_policy(show_dock_icon: bool) -> isize {
    if show_dock_icon {
        NS_APP_ACTIVATION_POLICY_REGULAR
    } else {
        NS_APP_ACTIVATION_POLICY_ACCESSORY
    }
}

#[cfg(target_os = "macos")]
fn dock_activation_policy_name(policy: isize) -> &'static str {
    match policy {
        NS_APP_ACTIVATION_POLICY_REGULAR => "Regular",
        NS_APP_ACTIVATION_POLICY_ACCESSORY => "Accessory",
        _ => "Unknown",
    }
}

#[cfg(target_os = "macos")]
unsafe fn set_dock_icon_for_app(shared_app: Id) {
    let Some(ns_data_class) = Class::get("NSData") else {
        warn!("set_dock_icon: NSData class not found");
        return;
    };
    let ns_data: Id = msg_send![
        ns_data_class,
        dataWithBytes: DOCK_ICON_BYTES.as_ptr()
        length: DOCK_ICON_BYTES.len()
    ];

    if ns_data.is_null() {
        warn!("set_dock_icon: failed to create NSData from icon bytes");
        return;
    }

    let Some(ns_image_class) = Class::get("NSImage") else {
        warn!("set_dock_icon: NSImage class not found");
        return;
    };
    let ns_image: Id = msg_send![ns_image_class, alloc];
    let ns_image: Id = msg_send![ns_image, initWithData: ns_data];

    if ns_image.is_null() {
        warn!("set_dock_icon: failed to create NSImage from icon data");
        return;
    }

    let _: () = msg_send![shared_app, setApplicationIconImage: ns_image];
    debug!("Dock icon image set successfully");
}

/// Set the Dock icon programmatically (for unbundled binaries)
///
/// This allows the app to show its custom icon in the Dock even when
/// running as a raw binary (not from a .app bundle).
pub fn set_dock_icon() {
    debug!("Setting Dock icon programmatically");

    Queue::main().exec_async(|| unsafe {
        let Some(ns_app_class) = Class::get("NSApplication") else {
            warn!("set_dock_icon: NSApplication class not found");
            return;
        };
        let shared_app: Id = msg_send![ns_app_class, sharedApplication];

        if shared_app.is_null() {
            warn!("set_dock_icon: NSApplication sharedApplication is null");
            return;
        }

        set_dock_icon_for_app(shared_app);
    });
}

/// Apply Dock visibility preference at runtime (best effort on macOS).
///
/// We switch NSApplication activation policy between:
/// - `Regular` (show Dock icon)
/// - `Accessory` (hide Dock icon, menu bar/tray style)
///
/// Some launch modes can refuse policy transitions (for example strict `LSUIElement`
/// behavior in certain app bundle contexts). In that case we keep current behavior and
/// only log a warning instead of failing.
#[cfg(target_os = "macos")]
pub fn apply_dock_icon_visibility(show_dock_icon: bool) {
    Queue::main().exec_async(move || unsafe {
        let Some(ns_app_class) = Class::get("NSApplication") else {
            warn!("apply_dock_icon_visibility: NSApplication class not found");
            return;
        };
        let shared_app: Id = msg_send![ns_app_class, sharedApplication];
        if shared_app.is_null() {
            warn!("apply_dock_icon_visibility: NSApplication sharedApplication is null");
            return;
        }

        let target_policy = dock_activation_policy(show_dock_icon);
        let current_policy: isize = msg_send![shared_app, activationPolicy];

        if current_policy != target_policy {
            let changed: bool = msg_send![shared_app, setActivationPolicy: target_policy];
            if !changed {
                warn!(
                    "Show dock icon={} requested but activation policy change {} -> {} was refused. \
                     Keeping current Dock behavior (likely launch-mode limitation).",
                    show_dock_icon,
                    dock_activation_policy_name(current_policy),
                    dock_activation_policy_name(target_policy),
                );
                return;
            }

            debug!(
                "Dock activation policy changed: {} -> {}",
                dock_activation_policy_name(current_policy),
                dock_activation_policy_name(target_policy),
            );
        }

        if show_dock_icon {
            set_dock_icon_for_app(shared_app);
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn apply_dock_icon_visibility(_show_dock_icon: bool) {}

/// Install a minimal AppKit main menu with standard Edit key equivalents.
///
/// CodeScribe runs as an `LSUIElement` agent app (no visible menu bar). In this mode AppKit still
/// relies on the app's `mainMenu` to resolve Command-key equivalents like Cmd+C / Cmd+V for text
/// controls (field editor). Without it, selectable text in bubbles and the Agent input field can
/// appear "dead" for copy/paste even though typing works.
///
/// This is safe to call multiple times.
#[cfg(target_os = "macos")]
pub fn install_basic_edit_menu() {
    use std::sync::Once;

    static INIT: Once = Once::new();
    INIT.call_once(|| {
        Queue::main().exec_async(|| unsafe {
            let ns_app_class = Class::get("NSApplication").expect("NSApplication class not found");
            let app: Id = msg_send![ns_app_class, sharedApplication];
            if app.is_null() {
                warn!("install_basic_edit_menu: NSApplication sharedApplication is null");
                return;
            }

            let ns_menu = Class::get("NSMenu").expect("NSMenu class not found");
            let ns_menu_item = Class::get("NSMenuItem").expect("NSMenuItem class not found");

            let main_menu: Id = msg_send![ns_menu, alloc];
            let main_menu: Id = msg_send![main_menu, init];

            // App menu (required for some key equivalent routing, even if hidden)
            let app_item: Id = msg_send![ns_menu_item, alloc];
            let app_item: Id = msg_send![app_item, init];
            let app_menu: Id = msg_send![ns_menu, alloc];
            let app_menu: Id = msg_send![app_menu, init];

            let quit_title = ns_string("Quit CodeScribe");
            let quit_key = ns_string("q");
            let quit_item: Id = msg_send![ns_menu_item, alloc];
            let quit_item: Id =
                msg_send![quit_item, initWithTitle: quit_title action: sel!(terminate:) keyEquivalent: quit_key];
            let _: () = msg_send![app_menu, addItem: quit_item];
            let _: () = msg_send![app_item, setSubmenu: app_menu];
            let _: () = msg_send![main_menu, addItem: app_item];

            // Edit menu with standard key equivalents
            let edit_item: Id = msg_send![ns_menu_item, alloc];
            let edit_item: Id = msg_send![edit_item, init];
            let _: () = msg_send![edit_item, setTitle: ns_string("Edit")];

            let edit_menu: Id = msg_send![ns_menu, alloc];
            let edit_menu: Id = msg_send![edit_menu, init];

            let make_edit = |title: &str, sel: Sel, key: &str| -> Id {
                let item: Id = msg_send![ns_menu_item, alloc];
                msg_send![
                    item,
                    initWithTitle: ns_string(title)
                    action: sel
                    keyEquivalent: ns_string(key)
                ]
            };

            let cut_item = make_edit("Cut", sel!(cut:), "x");
            let copy_item = make_edit("Copy", sel!(copy:), "c");
            let paste_item = make_edit("Paste", sel!(paste:), "v");
            let select_all_item = make_edit("Select All", sel!(selectAll:), "a");

            let _: () = msg_send![edit_menu, addItem: cut_item];
            let _: () = msg_send![edit_menu, addItem: copy_item];
            let _: () = msg_send![edit_menu, addItem: paste_item];
            let _: () = msg_send![edit_menu, addItem: select_all_item];

            let _: () = msg_send![edit_item, setSubmenu: edit_menu];
            let _: () = msg_send![main_menu, addItem: edit_item];

            let _: () = msg_send![app, setMainMenu: main_menu];
            debug!("install_basic_edit_menu: mainMenu installed");
        });
    });
}

#[cfg(not(target_os = "macos"))]
pub fn install_basic_edit_menu() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_position() {
        let (x, y) = get_cursor_position();
        // Just verify we get some coordinates
        assert!(x >= 0.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn test_focused_element_check() {
        // This will return false in test environment (no GUI)
        // but verifies the function doesn't crash
        let _ = focused_element_accepts_text();
    }

    #[test]
    fn test_badge_config_default() {
        let config = HoldBadgeConfig::default();
        assert_eq!(config.diameter, 12.0);
        assert_eq!(config.offset, (10.0, -10.0));
        assert_eq!(config.update_interval_ms, 150);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_dock_policy_mapping() {
        assert_eq!(
            dock_activation_policy(true),
            NS_APP_ACTIVATION_POLICY_REGULAR
        );
        assert_eq!(
            dock_activation_policy(false),
            NS_APP_ACTIVATION_POLICY_ACCESSORY
        );
    }

    #[test]
    fn test_apply_dock_icon_visibility_is_safe_to_call() {
        apply_dock_icon_visibility(true);
        apply_dock_icon_visibility(false);
    }
}
