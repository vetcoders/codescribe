//! Cursor-following "hold badge" recording indicator.
//!
//! A small colored dot that floats near the text caret (via the Accessibility
//! API) and falls back to the mouse cursor when no caret is available. Used to
//! signal recording / processing / assistive states during dictation.
//!
//! Resurrected as a self-contained `app/os` module (previously lived in the now
//! excised `app/ui` AppKit layer). All AppKit/objc code is macOS-only; a no-op
//! stub surface keeps non-macOS builds compiling.
//!
//! Vibecrafted with AI Agents by Vetcoders (c)2026 Vetcoders

// ─────────────────────────────────────────────────────────────
// Platform-agnostic surface (pure data, no AppKit)
// ─────────────────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::{BadgeMode, HoldBadgeConfig};

    #[test]
    fn hold_badge_modes_encode_processing_and_assistive_affordances() {
        assert_eq!(BadgeMode::Processing.color(), (1.0, 0.5, 0.0, 0.85));
        assert!(BadgeMode::Processing.should_pulse());
        assert!(!BadgeMode::Processing.has_glow());
        assert_eq!(BadgeMode::Processing.diameter_multiplier(), 1.0);

        assert_eq!(BadgeMode::Assistive.color(), (0.6, 0.2, 0.9, 0.85));
        assert!(!BadgeMode::Assistive.should_pulse());
        assert!(BadgeMode::Assistive.has_glow());
        assert_eq!(BadgeMode::Assistive.diameter_multiplier(), 1.2);
    }

    #[test]
    fn hold_badge_config_from_mode_preserves_layout_and_applies_mode_visuals() {
        let base = HoldBadgeConfig::default();
        let processing = HoldBadgeConfig::from_mode(BadgeMode::Processing);
        assert_eq!(processing.mode, BadgeMode::Processing);
        assert_eq!(processing.color, BadgeMode::Processing.color());
        assert_eq!(processing.diameter, base.diameter);
        assert_eq!(processing.offset, base.offset);
        assert_eq!(processing.update_interval_ms, base.update_interval_ms);

        let assistive = HoldBadgeConfig::from_mode(BadgeMode::Assistive);
        assert_eq!(assistive.mode, BadgeMode::Assistive);
        assert_eq!(assistive.color, BadgeMode::Assistive.color());
        assert_eq!(
            assistive.diameter,
            base.diameter * BadgeMode::Assistive.diameter_multiplier()
        );
        assert_eq!(assistive.offset, base.offset);
        assert_eq!(assistive.update_interval_ms, base.update_interval_ms);
    }
}

// ─────────────────────────────────────────────────────────────
// macOS implementation
// ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod imp {
    use super::{BadgeMode, HoldBadgeConfig};

    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use dispatch::Queue;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use std::ptr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tracing::{debug, warn};

    use crate::os::Id;

    // Accessibility API bindings (use raw pointers compatible with C FFI)
    type AXId = *mut std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCopyAttributeValue(element: AXId, attribute: AXId, value: *mut AXId) -> i32;
        fn AXUIElementCreateSystemWide() -> AXId;
        fn AXValueGetValue(value: AXId, type_: i32, value_ptr: *mut std::ffi::c_void) -> bool;
        fn CFRelease(cf: *const std::ffi::c_void);
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

    // AX constants
    const AX_ERROR_SUCCESS: i32 = 0;
    const AX_FOCUSED_UIELEMENT_ATTRIBUTE: &str = "AXFocusedUIElement";
    const AX_ROLE_ATTRIBUTE: &str = "AXRole";
    const AX_SELECTED_TEXT_RANGE_ATTRIBUTE: &str = "AXSelectedTextRange";
    const AX_POSITION_ATTRIBUTE: &str = "AXPosition";
    const AX_SIZE_ATTRIBUTE: &str = "AXSize";

    // AXValue types
    const AX_VALUE_CGPOINT_TYPE: i32 = 1;
    const AX_VALUE_CGSIZE_TYPE: i32 = 2;
    const AX_VALUE_CFRANGE_TYPE: i32 = 3;

    // Window level constants
    const NS_STATUS_WINDOW_LEVEL: i64 = 25;

    // ── inlined AppKit helpers (were `ui::shared::helpers::shell`) ──

    /// Add subview to a view.
    /// # Safety
    /// `parent` and `child` must be valid Objective-C views.
    unsafe fn add_subview(parent: Id, child: Id) {
        unsafe {
            let _: () = msg_send![parent, addSubview: child];
        }
    }

    /// Show panel (order front, even when app is inactive).
    /// # Safety
    /// `panel` must be a valid `NSPanel` / `NSWindow` instance.
    unsafe fn panel_show(panel: Id) {
        unsafe {
            let _: () = msg_send![panel, orderFrontRegardless];
        }
    }

    /// Close panel.
    /// # Safety
    /// `panel` must be a valid `NSPanel` / `NSWindow` instance.
    unsafe fn panel_close(panel: Id) {
        unsafe {
            let _: () = msg_send![panel, close];
        }
    }

    /// Hold badge state
    struct HoldBadgeState {
        window: Option<usize>, // Stores the NSPanel pointer as usize to make it Send.
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

    /// Monotonic show generation. A show captures it at REQUEST time; `hide_hold_badge`
    /// bumps it. When a queued (`exec_async`) show finally runs on the main thread it
    /// aborts if the generation moved — otherwise a hide that raced ahead of the
    /// enqueued show would be undone, leaving a badge panel + updater thread stuck
    /// with nothing to tear them down.
    static BADGE_GENERATION: AtomicU64 = AtomicU64::new(0);

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

            // Extract range. Populated by `AXValueGetValue` through an out-pointer;
            // fields are written by the framework, so `dead_code` on them is spurious.
            #[repr(C)]
            #[allow(dead_code)]
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

    /// Get the current mouse cursor position in screen coordinates
    pub fn get_cursor_position() -> (f64, f64) {
        let mouse_location = NSEvent::mouseLocation();
        (mouse_location.x, mouse_location.y)
    }

    /// Get the best available position for the badge (caret or cursor)
    fn get_badge_position() -> (f64, f64) {
        get_caret_position().unwrap_or_else(get_cursor_position)
    }

    /// Create a CGColor from RGBA components
    /// # Safety
    /// Returns a `+1` retained `CGColorRef`; caller must `CGColorRelease` it.
    unsafe fn create_cg_color(r: f64, g: f64, b: f64, a: f64) -> *const std::ffi::c_void {
        unsafe {
            let color_space = CGColorSpaceCreateDeviceRGB();
            let components: [f64; 4] = [r, g, b, a];
            let color = CGColorCreate(color_space, components.as_ptr());
            CGColorSpaceRelease(color_space);
            color
        }
    }

    /// Create the circular badge view using CALayer for reliable rendering
    /// # Safety
    /// Must run on the main thread; returns a `+1` retained `NSView`.
    unsafe fn create_badge_view(config: &HoldBadgeConfig) -> Id {
        unsafe {
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

            // Configure the layer to draw a circle.
            // Set background color from config (default: red with 80% opacity).
            let cg_color = create_cg_color(
                config.color.0,
                config.color.1,
                config.color.2,
                config.color.3,
            );
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            CGColorRelease(cg_color);

            // Make it circular by setting corner radius to half the diameter
            let corner_radius = config.diameter / 2.0;
            let _: () = msg_send![layer, setCornerRadius: corner_radius];

            // Ensure the layer clips to bounds (for the circle shape)
            let _: () = msg_send![layer, setMasksToBounds: true];

            view
        }
    }

    /// Create the hold badge panel
    /// # Safety
    /// Must run on the main thread; returns a `+1` retained non-activating `NSPanel`.
    unsafe fn create_badge_panel(config: &HoldBadgeConfig) -> Id {
        unsafe {
            let ns_panel = Class::get("NSPanel").unwrap();

            // Get initial position
            let (x, y) = get_badge_position();
            let adjusted_x = x + config.offset.0;
            let adjusted_y = y + config.offset.1;
            debug!(
                "Badge position: raw=({:.1}, {:.1}), adjusted=({:.1}, {:.1}), diameter={}",
                x, y, adjusted_x, adjusted_y, config.diameter
            );

            // Create panel frame using CGRect (screen coordinates)
            let panel_frame = CGRect {
                origin: CGPoint {
                    x: adjusted_x,
                    y: adjusted_y,
                },
                size: CGSize {
                    width: config.diameter,
                    height: config.diameter,
                },
            };

            // Create a non-activating panel so the badge never steals app/key focus.
            let panel: Id = msg_send![ns_panel, alloc];
            let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
            let backing = NSBackingStoreType::Buffered;
            let panel: Id = msg_send![
                panel,
                initWithContentRect: panel_frame
                styleMask: style_mask
                backing: backing
                defer: false
            ];

            // Configure panel for floating transparent overlay.
            let clear_color = NSColor::clearColor();
            let clear_color_ptr = &*clear_color as *const _ as Id;
            let _: () = msg_send![panel, setOpaque: false];
            let _: () = msg_send![panel, setBackgroundColor: clear_color_ptr];
            let _: () = msg_send![panel, setIgnoresMouseEvents: true];
            let _: () = msg_send![panel, setHidesOnDeactivate: false];
            let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
            let _: () = msg_send![panel, setFloatingPanel: true];
            // Status-window level (25) is above floating level (3), preserving the
            // previous always-on-top behavior while keeping this surface a panel.
            let _: () = msg_send![panel, setLevel: NS_STATUS_WINDOW_LEVEL];
            // Ensure the helper panel shows over fullscreen Spaces and stays out of cycling.
            let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle;
            let _: () = msg_send![panel, setCollectionBehavior: collection_behavior];

            // Enable layer-backed views for better transparency/compositing
            let content_view: Id = msg_send![panel, contentView];
            let _: () = msg_send![content_view, setWantsLayer: true];

            // Create badge view (circular colored indicator)
            let badge_view = create_badge_view(config);
            add_subview(content_view, badge_view);

            // Force the view to display
            let _: () = msg_send![badge_view, setNeedsDisplay: true];

            panel
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

    /// Internal implementation that must run on the main thread.
    ///
    /// `generation` is the show generation captured when this show was requested.
    /// If a `hide_hold_badge` bumped it in the meantime, the show is stale and is
    /// aborted so it cannot resurrect a badge the user already dismissed.
    fn show_hold_badge_impl(config: HoldBadgeConfig, generation: u64) {
        // A hide issued after this show was enqueued already tore the badge down;
        // honor it and do not create a new panel/updater.
        if generation != BADGE_GENERATION.load(Ordering::SeqCst) {
            debug!("Skipping stale hold-badge show (superseded by hide)");
            return;
        }
        debug!("Showing hold badge (diameter={})", config.diameter);
        unsafe {
            // IMPORTANT: do not hold BADGE_STATE while calling `panel_close`.
            // Closing a panel can trigger AppKit callbacks/notifications which may
            // re-enter our code and attempt to lock BADGE_STATE again → deadlock.
            let old_panel_ptr = {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.window.take()
            };
            if let Some(panel_ptr) = old_panel_ptr {
                panel_close(panel_ptr as Id);
            }

            // Create new badge panel (MUST be on main thread)
            let panel = create_badge_panel(&config);

            // Make panel visible without activating the app.
            panel_show(panel);

            // Force content view to redraw
            let content_view: Id = msg_send![panel, contentView];
            let _: () = msg_send![content_view, setNeedsDisplay: true];

            // Update shared state and determine whether we need to start the updater thread.
            let update_interval = config.update_interval_ms;
            let start_updater;
            {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                // Re-check under the lock: a hide that landed between the top-of-fn
                // check and here bumped the generation. Honor it — drop the lock,
                // close the panel we just created (never hold BADGE_STATE across
                // panel_close, see above), and leave the state torn down.
                if generation != BADGE_GENERATION.load(Ordering::SeqCst) {
                    drop(state);
                    panel_close(panel);
                    debug!("Aborting hold-badge show; hide raced in during panel creation");
                    return;
                }
                let was_running = state.timer_running;
                state.window = Some(panel as usize);
                state.config = config.clone();
                state.timer_running = true;
                start_updater = !was_running;
            }

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
                        let first_sample = lx.is_nan();
                        let moved_x = (new_x - lx).abs() > 2.0;
                        let moved_y = (new_y - ly).abs() > 2.0;
                        let position_changed = first_sample || moved_x || moved_y;

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
                                        let badge_view: Id =
                                            msg_send![subviews, objectAtIndex: 0usize];
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

    /// Show the hold badge with custom configuration.
    /// This dispatches to the main thread for thread safety with AppKit.
    pub fn show_hold_badge_with_config(config: HoldBadgeConfig) {
        // Capture the show generation at REQUEST time. A hide that lands before the
        // (possibly queued) impl runs bumps the generation, so the impl recognises
        // itself as stale and does not resurrect a dismissed badge.
        let generation = BADGE_GENERATION.load(Ordering::SeqCst);

        // Check if we're already on the main thread by checking thread name.
        // Note: exec_sync on main queue from main thread causes deadlock.
        let is_main_thread = std::thread::current().name() == Some("main");

        if is_main_thread {
            show_hold_badge_impl(config, generation);
        } else {
            // Dispatch to main thread - AppKit panel creation MUST be on main thread.
            // Using exec_async to avoid deadlock when called from tokio runtime.
            Queue::main().exec_async(move || {
                show_hold_badge_impl(config, generation);
            });
        }
    }

    /// Hide the hold badge and stop position tracking.
    /// This dispatches to the main thread for thread safety with AppKit.
    pub fn hide_hold_badge() {
        debug!("Hiding hold badge");

        // Bump the show generation so any show enqueued before this hide (or racing
        // its panel creation) is recognised as stale and does not resurrect the
        // badge after we tear it down.
        BADGE_GENERATION.fetch_add(1, Ordering::SeqCst);

        // Stop the timer first (can be done on any thread)
        let panel_ptr = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.timer_running = false;
            state.window.take()
        };

        // Dispatch panel close to main thread. Do NOT hold BADGE_STATE while closing.
        Queue::main().exec_async(move || {
            if let Some(panel_ptr) = panel_ptr {
                unsafe {
                    panel_close(panel_ptr as Id);
                }
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    focused_element_accepts_text, get_caret_position, get_cursor_position, hide_hold_badge,
    show_badge_for_mode, show_hold_badge, show_hold_badge_with_config,
};

// ─────────────────────────────────────────────────────────────
// Non-macOS no-op stubs (keep the public surface compiling)
// ─────────────────────────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
mod stubs {
    use super::{BadgeMode, HoldBadgeConfig};

    /// No-op: focused-element text detection is macOS-only.
    pub fn focused_element_accepts_text() -> bool {
        false
    }

    /// No-op: caret tracking is macOS-only.
    pub fn get_caret_position() -> Option<(f64, f64)> {
        None
    }

    /// No-op: cursor tracking is macOS-only.
    pub fn get_cursor_position() -> (f64, f64) {
        (0.0, 0.0)
    }

    /// No-op on non-macOS platforms.
    pub fn show_hold_badge() {}

    /// No-op on non-macOS platforms.
    pub fn show_badge_for_mode(_mode: BadgeMode) {}

    /// No-op on non-macOS platforms.
    pub fn show_hold_badge_with_config(_config: HoldBadgeConfig) {}

    /// No-op on non-macOS platforms.
    pub fn hide_hold_badge() {}
}

#[cfg(not(target_os = "macos"))]
pub use stubs::{
    focused_element_accepts_text, get_caret_position, get_cursor_position, hide_hold_badge,
    show_badge_for_mode, show_hold_badge, show_hold_badge_with_config,
};
