//! Native AppKit UI helpers for CodeScribe
//!
//! Reduces msg_send! boilerplate by providing high-level functions for common UI patterns.
//! These helpers wrap Objective-C calls in safe, reusable Rust functions.
//!
//! # Safety
//! All functions in this module operate on raw Objective-C pointers (`Id = *mut Object`).
//! Callers must ensure pointers are valid. This is standard for Rust-ObjC FFI.

use core_graphics::geometry::{CGRect, CGSize};
use objc::runtime::Sel;
use objc::runtime::{Class, Object, class_getInstanceMethod, object_getClass};
use objc::{msg_send, sel, sel_impl};
#[cfg(feature = "liquid_glass")]
use objc2::MainThreadMarker;
#[cfg(feature = "liquid_glass")]
use objc2::rc::Retained;
#[cfg(feature = "liquid_glass")]
use objc2_app_kit::{NSAppKitVersionNumber, NSGlassEffectView, NSGlassEffectViewStyle};
use objc2_app_kit::{NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState};
#[cfg(feature = "liquid_glass")]
use objc2_core_foundation::{
    CGPoint as Objc2CGPoint, CGRect as Objc2CGRect, CGSize as Objc2CGSize,
};
use std::ffi::CString;
use std::sync::{Once, OnceLock};

/// Type alias for Objective-C object pointers
pub type Id = *mut Object;

/// Window level constants
pub const NS_FLOATING_WINDOW_LEVEL: i64 = 3;
pub const NS_STATUS_WINDOW_LEVEL: i64 = 25;
pub const NS_NORMAL_WINDOW_LEVEL: i64 = 0;

/// Focus ring constants
pub const NS_FOCUS_RING_TYPE_DEFAULT: i64 = 0;
pub const NS_FOCUS_RING_TYPE_NONE: i64 = 1;
pub const NS_FOCUS_RING_TYPE_EXTERIOR: i64 = 2;

mod chat_views;
mod controls;
mod file_actions;
mod inputs;
mod layout;
mod shell;
pub use chat_views::*;
pub use controls::*;
pub use file_actions::*;
pub use inputs::*;
pub use layout::*;
pub use shell::*;

// ============================================================================
// UI Tokens (shared sizes/spacing; aligned to Settings)
// ============================================================================

pub mod ui_tokens {
    pub const TITLE_FONT_SIZE: f64 = 15.0;
    pub const BODY_FONT_SIZE: f64 = 13.0;
    pub const SMALL_FONT_SIZE: f64 = 11.0;
    pub const MICRO_FONT_SIZE: f64 = 10.0;

    pub const HEADER_HEIGHT: f64 = 44.0;
    pub const FOOTER_HEIGHT: f64 = 40.0;
    /// Slim brand-only footer for the voice-chat window: just tall enough for
    /// the watermark label, so chat content scrolls beneath the input bar to
    /// (almost) the window edge instead of clipping high above it.
    pub const CHAT_FOOTER_HEIGHT: f64 = 20.0;
    pub const EDGE_PADDING: f64 = 16.0;
    pub const EDGE_PADDING_TIGHT: f64 = 12.0;

    pub const TITLE_LABEL_WIDTH: f64 = 96.0;
    pub const CHAT_TITLE_LABEL_WIDTH: f64 = 104.0;
    pub const TRAFFIC_LIGHTS_SPACER_WIDTH: f64 = 80.0;
    pub const HEADER_BUTTON_SIZE: f64 = 28.0;
    pub const HEADER_BUTTON_GAP: f64 = 8.0;
    pub const CHAT_HEADER_BUTTON_SIZE: f64 = 26.0;
    pub const CHAT_HEADER_BUTTON_GAP: f64 = 6.0;
    pub const CHAT_HEADER_GROUP_GAP: f64 = 8.0;
    pub const CHAT_TAB_BUTTON_MIN_WIDTH: f64 = 22.0;
    pub const CHAT_TAB_BUTTON_MIN_GAP: f64 = 3.0;
    pub const CHAT_TAB_BUTTON_GAP: f64 = 4.0;
    pub const CHAT_TAB_BUTTON_COLLAPSED_WIDTH: f64 = 18.0;
    pub const HELP_PANEL_WIDTH: f64 = 150.0;
    pub const FOOTER_INSET: f64 = 4.0;
    pub const AGENT_INPUT_HEIGHT: f64 = 44.0;
    pub const CONTENT_GAP: f64 = 4.0;
    pub const SIDEBAR_MIN_WIDTH: f64 = 200.0;
    pub const SIDEBAR_MAX_WIDTH: f64 = 320.0;
    pub const CHAT_INPUT_BUTTON_WIDTH: f64 = 36.0;
    pub const CHAT_INPUT_BUTTON_HEIGHT: f64 = 32.0;
    pub const CHAT_INPUT_SIDE_INSET: f64 = 8.0;
    pub const CHAT_INPUT_CONTROL_GAP: f64 = 8.0;
    pub const CHAT_INPUT_TEXT_INSET_Y: f64 = 7.0;

    // Legacy CORNER_RADIUS_LG/MD/SM removed — use SURFACE_RADIUS everywhere.

    pub const STATUS_PILL_HEIGHT: f64 = 18.0;
    pub const STATUS_PILL_WIDTH: f64 = 96.0;
    pub const STATUS_PILL_MIN_WIDTH: f64 = 68.0;
    pub const STATUS_PILL_DOT_INSET_X: f64 = 6.0;
    pub const STATUS_PILL_LABEL_INSET_X: f64 = 14.0;
    pub const STATUS_PILL_LABEL_INSET_RIGHT: f64 = 4.0;
    pub const STATUS_DOT_SIZE: f64 = 5.0;
    pub const BUBBLE_MAX_WIDTH: f64 = 560.0;

    pub const PLACEHOLDER_LINE_WIDTH: f64 = 120.0;
    pub const PLACEHOLDER_LINE_HEIGHT: f64 = 2.0;

    pub const EMPTY_STATE_HEIGHT: f64 = 160.0;
    pub const EMPTY_STATE_BUTTON_HEIGHT: f64 = 28.0;
    pub const EMPTY_STATE_BUTTON_WIDTH: f64 = 140.0;
    pub const EMPTY_STATE_BUTTON_GAP: f64 = 12.0;

    pub const DRAWER_ROW_WIDTH: f64 = 410.0;
    pub const DRAWER_ROW_HEIGHT: f64 = 58.0;
    pub const DRAWER_ROW_RADIUS: f64 = 8.0;
    pub const DRAWER_ROW_PAD_X: f64 = 10.0;
    pub const DRAWER_BADGE_WIDTH: f64 = 62.0;
    pub const DRAWER_BADGE_HEIGHT: f64 = 16.0;
    pub const DRAWER_ACTION_BUTTON_SIZE: f64 = 19.0;
    pub const DRAWER_ACTION_BUTTON_GAP: f64 = 2.0;
    pub const DRAWER_ACTION_RIGHT_INSET: f64 = 18.0;
    pub const DRAWER_SECTION_HEADER_HEIGHT: f64 = 22.0;

    // ─── Tafla: unified surface design system ──────────────────────
    // Glass = frame (cool, system materials).  Paper = content (warm, readable).
    // One radius, one border, no stacking — flat pane, not bubble soup.

    /// Canonical corner radius — use this everywhere instead of LG/MD/SM mix.
    pub const SURFACE_RADIUS: f64 = 12.0;
    pub const SURFACE_BORDER_WIDTH: f64 = 1.0;
    pub const SURFACE_BORDER_ALPHA: f64 = 0.14;

    /// Glass background: alpha for vibrancy-backed views.
    pub const GLASS_BG_ALPHA: f64 = 0.24;
    /// Glass fallback: alpha when NSVisualEffectView is not available.
    pub const GLASS_FALLBACK_ALPHA: f64 = 0.34;

    /// Paper tiers are appearance-aware: derived from controlBackgroundColor.
    pub const PAPER_WARM_ALPHA: f64 = 0.74;
    pub const PAPER_WARM_FALLBACK_ALPHA: f64 = 0.84;
    /// Paper cool tier (system/meta areas).
    pub const PAPER_COOL_ALPHA: f64 = 0.70;
    pub const PAPER_COOL_FALLBACK_ALPHA: f64 = 0.80;
    pub const PAPER_BORDER_ALPHA: f64 = 0.14;

    /// Compact header for Tafla windows.
    pub const HEADER_HEIGHT_COMPACT: f64 = 46.0;
    pub const HEADER_BORDER_ALPHA: f64 = 0.10;
    pub const SETTINGS_WINDOW_OPACITY: f64 = 1.00;

    /// Tafla density tiers (vertical gap between controls per tab density).
    pub const DENSITY_COMFORTABLE: f64 = 12.0;
    pub const DENSITY_MEDIUM: f64 = 8.0;
    pub const DENSITY_COMPACT: f64 = 6.0;

    /// Extra vertical gap inserted above section headers within settings tabs.
    pub const SECTION_GAP: f64 = 20.0;

    /// Dictation overlay tuning: lighter sheet + compact action row.
    pub const OVERLAY_GLASS_BG_ALPHA: f64 = 0.18;
    pub const OVERLAY_GLASS_FALLBACK_ALPHA: f64 = 0.28;
    pub const OVERLAY_BORDER_ALPHA: f64 = 0.10;
    pub const OVERLAY_TEXT_PANEL_ALPHA: f64 = 0.74;
    pub const OVERLAY_ACTION_BG_ALPHA: f64 = 0.70;
    pub const OVERLAY_ACTION_BORDER_ALPHA: f64 = 0.12;
    pub const OVERLAY_ACTION_BUTTON_WIDTH: f64 = 84.0;
    pub const OVERLAY_ACTION_BUTTON_HEIGHT: f64 = 24.0;
}

// ============================================================================
// Color Helpers
// ============================================================================

pub mod ui_colors {
    use super::Id;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    fn with_alpha(color: Id, alpha: f64) -> Id {
        unsafe { msg_send![color, colorWithAlphaComponent: alpha] }
    }

    fn adaptive_alpha(glass_alpha: f64, fallback_alpha: f64) -> f64 {
        if super::glass_effect_supported() {
            glass_alpha
        } else {
            fallback_alpha
        }
    }

    pub fn sidebar_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.22, 0.32))
    }

    pub fn panel_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.28, 0.38))
    }

    pub fn settings_glass_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.26, 0.36))
    }

    pub fn input_bar_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.22, 0.32))
    }

    pub fn input_bar_border() -> Id {
        header_border()
    }

    pub fn overlay_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, labelColor];
            with_alpha(base, 0.92)
        }
    }

    pub fn overlay_hint_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, secondaryLabelColor];
            with_alpha(base, 0.7)
        }
    }

    pub fn overlay_sheet_bg() -> Id {
        use super::ui_tokens::{OVERLAY_GLASS_BG_ALPHA, OVERLAY_GLASS_FALLBACK_ALPHA};
        control_bg_tint(adaptive_alpha(
            OVERLAY_GLASS_BG_ALPHA,
            OVERLAY_GLASS_FALLBACK_ALPHA,
        ))
    }

    pub fn overlay_sheet_border() -> Id {
        use super::ui_tokens::OVERLAY_BORDER_ALPHA;
        with_alpha(separator(), OVERLAY_BORDER_ALPHA)
    }

    pub fn overlay_text_panel_bg() -> Id {
        use super::ui_tokens::OVERLAY_TEXT_PANEL_ALPHA;
        with_alpha(surface_paper_warm(), OVERLAY_TEXT_PANEL_ALPHA)
    }

    pub fn overlay_action_bg() -> Id {
        use super::ui_tokens::OVERLAY_ACTION_BG_ALPHA;
        with_alpha(surface_paper_warm(), OVERLAY_ACTION_BG_ALPHA)
    }

    pub fn overlay_action_border() -> Id {
        use super::ui_tokens::OVERLAY_ACTION_BORDER_ALPHA;
        with_alpha(separator(), OVERLAY_ACTION_BORDER_ALPHA)
    }

    pub fn separator() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, separatorColor]
        }
    }

    pub fn secondary_label() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, secondaryLabelColor]
        }
    }

    pub fn control_bg_tint(alpha: f64) -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, controlBackgroundColor];
            with_alpha(base, alpha)
        }
    }

    pub fn accent() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, controlAccentColor]
        }
    }

    pub fn accent_tint(alpha: f64) -> Id {
        with_alpha(accent(), alpha)
    }

    pub fn status_granted() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemGreenColor]
        }
    }

    pub fn status_denied() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemRedColor]
        }
    }

    pub fn status_warning() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemOrangeColor]
        }
    }

    pub fn card_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.24, 0.34))
    }

    pub fn empty_state_bg() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, controlBackgroundColor];
            with_alpha(base, adaptive_alpha(0.26, 0.36))
        }
    }

    pub fn search_highlight_bg() -> Id {
        accent_tint(0.20)
    }

    pub fn bubble_user_bg() -> Id {
        accent_tint(0.10)
    }

    pub fn bubble_user_border() -> Id {
        accent_tint(0.22)
    }

    pub fn bubble_assistant_bg() -> Id {
        surface_paper_warm()
    }

    pub fn bubble_system_bg() -> Id {
        surface_paper_cool()
    }

    pub fn bubble_border() -> Id {
        use super::ui_tokens::PAPER_BORDER_ALPHA;
        with_alpha(separator(), PAPER_BORDER_ALPHA)
    }

    pub fn bubble_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, labelColor]
        }
    }

    pub fn bubble_meta_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, secondaryLabelColor];
            with_alpha(base, 0.82)
        }
    }

    pub fn bubble_streaming_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, secondaryLabelColor];
            with_alpha(base, 0.82)
        }
    }

    pub fn bubble_error_bg() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            let base: Id = msg_send![ns_color, systemRedColor];
            with_alpha(base, 0.12)
        }
    }

    pub fn bubble_error_text() -> Id {
        unsafe {
            let ns_color = Class::get("NSColor").unwrap();
            msg_send![ns_color, systemRedColor]
        }
    }

    // ─── Tafla: unified surface colors ─────────────────────────────

    /// Glass surface background (panels, sidebar, overlays with vibrancy).
    pub fn surface_glass() -> Id {
        use super::ui_tokens::{GLASS_BG_ALPHA, GLASS_FALLBACK_ALPHA};
        control_bg_tint(adaptive_alpha(GLASS_BG_ALPHA, GLASS_FALLBACK_ALPHA))
    }

    /// Paper warm surface (content: bubbles, transcription text, input fields).
    pub fn surface_paper_warm() -> Id {
        use super::ui_tokens::{PAPER_WARM_ALPHA, PAPER_WARM_FALLBACK_ALPHA};
        control_bg_tint(adaptive_alpha(PAPER_WARM_ALPHA, PAPER_WARM_FALLBACK_ALPHA))
    }

    /// Paper cool surface (system/meta content).
    pub fn surface_paper_cool() -> Id {
        use super::ui_tokens::{PAPER_COOL_ALPHA, PAPER_COOL_FALLBACK_ALPHA};
        control_bg_tint(adaptive_alpha(PAPER_COOL_ALPHA, PAPER_COOL_FALLBACK_ALPHA))
    }

    /// Canonical surface border — one style for all Tafla windows.
    pub fn surface_border() -> Id {
        use super::ui_tokens::SURFACE_BORDER_ALPHA;
        with_alpha(separator(), SURFACE_BORDER_ALPHA)
    }

    /// Header bottom separator — subtler than surface border.
    pub fn header_border() -> Id {
        use super::ui_tokens::HEADER_BORDER_ALPHA;
        with_alpha(separator(), HEADER_BORDER_ALPHA)
    }
}

/// Apply Tafla surface treatment to a CALayer: corner radius + optional border.
/// Shadows off by default — Tafla is flat pane, not bubble soup.
///
/// # Safety
/// `layer` must be a valid pointer to a CALayer (or NSView.layer).
pub unsafe fn apply_tafla_surface(layer: Id, with_border: bool) {
    let _: () = msg_send![layer, setCornerRadius: ui_tokens::SURFACE_RADIUS];
    if with_border {
        let border = ui_colors::surface_border();
        let cg_border: Id = msg_send![border, CGColor];
        let _: () = msg_send![layer, setBorderColor: cg_border];
        let _: () = msg_send![layer, setBorderWidth: ui_tokens::SURFACE_BORDER_WIDTH];
    }
    let _: () = msg_send![layer, setShadowOpacity: 0.0f32];
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NSEdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

#[cfg(feature = "liquid_glass")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NSOperatingSystemVersion {
    major_version: isize,
    minor_version: isize,
    patch_version: isize,
}

/// Create an NSColor from RGBA values (0.0-1.0)
pub fn color_rgba(r: f64, g: f64, b: f64, a: f64) -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, colorWithRed: r green: g blue: b alpha: a]
    }
}

/// Create clear (transparent) color
pub fn color_clear() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, clearColor]
    }
}

/// Create label color (dynamic in light/dark).
pub fn color_label() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, labelColor]
    }
}

/// Create secondary label color (dynamic in light/dark).
pub fn color_secondary_label() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, secondaryLabelColor]
    }
}

/// Layout insets for a view using Tahoe's layoutRegionGuides API.
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn layout_insets_for_view(view: Id) -> NSEdgeInsets {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };

    if let Some(frame) = unsafe { layout_region_frame_for_view(view) } {
        return insets_from_frame(bounds, frame);
    }

    unsafe { msg_send![view, safeAreaInsets] }
}

/// Layout region frame for a view (Tahoe layoutRegionGuides → contentLayoutGuide).
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn layout_region_frame_for_view(view: Id) -> Option<CGRect> {
    let guide = unsafe { layout_region_guide_for_view(view) }?;
    unsafe { layout_guide_frame(guide) }
}

/// Layout region guide using Tahoe's layoutRegionGuides API.
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn layout_region_guide_for_view(view: Id) -> Option<Id> {
    if view.is_null() {
        return None;
    }
    let cls = unsafe { object_getClass(view as *const Object) };
    if cls.is_null() {
        return None;
    }
    // Tahoe can expose NSGlassEffectView without this selector; avoid unrecognized selector crash.
    if unsafe { class_getInstanceMethod(cls, sel!(layoutRegionGuides)) }.is_null() {
        return None;
    }

    let guides: Id = unsafe { msg_send![view, layoutRegionGuides] };
    if !guides.is_null() {
        let guide: Id = unsafe { msg_send![guides, contentLayoutGuide] };
        if !guide.is_null() {
            return Some(guide);
        }
        let guide: Id = unsafe { msg_send![guides, safeAreaLayoutGuide] };
        if !guide.is_null() {
            return Some(guide);
        }
    }
    None
}

unsafe fn layout_guide_frame(guide: Id) -> Option<CGRect> {
    if guide.is_null() {
        return None;
    }
    let frame: CGRect = unsafe { msg_send![guide, layoutFrame] };
    Some(frame)
}

fn insets_from_frame(bounds: CGRect, frame: CGRect) -> NSEdgeInsets {
    let bounds_max_x = bounds.origin.x + bounds.size.width;
    let bounds_max_y = bounds.origin.y + bounds.size.height;
    let frame_max_x = frame.origin.x + frame.size.width;
    let frame_max_y = frame.origin.y + frame.size.height;

    let left = (frame.origin.x - bounds.origin.x).max(0.0);
    let bottom = (frame.origin.y - bounds.origin.y).max(0.0);
    let right = (bounds_max_x - frame_max_x).max(0.0);
    let top = (bounds_max_y - frame_max_y).max(0.0);

    NSEdgeInsets {
        top,
        left,
        bottom,
        right,
    }
}

#[cfg(feature = "liquid_glass")]
const NS_APPKIT_VERSION_26_0: f64 = 2685.0;

#[cfg(feature = "liquid_glass")]
fn glass_effect_style_for_material(material: NSVisualEffectMaterial) -> NSGlassEffectViewStyle {
    match material {
        // Titlebar stays Clear (light, floating chrome).
        // Sidebar uses Regular for readability — matches System Settings behaviour.
        NSVisualEffectMaterial::Titlebar => NSGlassEffectViewStyle::Clear,
        _ => NSGlassEffectViewStyle::Regular,
    }
}

fn glass_effect_view_class_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| Class::get("NSGlassEffectView").is_some())
}

#[cfg(feature = "liquid_glass")]
#[derive(Clone, Copy)]
struct GlassRuntimeProbe {
    appkit_version: f64,
    appkit_supported: bool,
    class_available: bool,
    os_version: NSOperatingSystemVersion,
    os_supported: bool,
}

#[cfg(feature = "liquid_glass")]
impl GlassRuntimeProbe {
    fn runtime_supported(self) -> bool {
        self.appkit_supported || self.os_supported
    }
}

#[cfg(feature = "liquid_glass")]
fn probe_glass_runtime() -> GlassRuntimeProbe {
    unsafe {
        let appkit_version = NSAppKitVersionNumber;
        let appkit_supported = appkit_version >= NS_APPKIT_VERSION_26_0;
        let os_version = Class::get("NSProcessInfo")
            .map(|cls| {
                let process_info: Id = msg_send![cls, processInfo];
                if process_info.is_null() {
                    NSOperatingSystemVersion::default()
                } else {
                    let version: NSOperatingSystemVersion =
                        msg_send![process_info, operatingSystemVersion];
                    version
                }
            })
            .unwrap_or_default();
        let os_supported = os_version.major_version >= 26;

        GlassRuntimeProbe {
            appkit_version,
            appkit_supported,
            class_available: glass_effect_view_class_available(),
            os_version,
            os_supported,
        }
    }
}

#[cfg(feature = "liquid_glass")]
fn glass_effect_runtime_supported() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| probe_glass_runtime().runtime_supported())
}

#[cfg(not(feature = "liquid_glass"))]
fn glass_effect_runtime_supported() -> bool {
    false
}

#[cfg(feature = "liquid_glass")]
fn create_typed_glass_effect_view(frame: CGRect, material: NSVisualEffectMaterial) -> Option<Id> {
    let runtime = probe_glass_runtime();
    if !runtime.runtime_supported() || !runtime.class_available {
        tracing::info!(
            "NSGlassEffectView fallback to NSVisualEffectView: runtime_supported={} appkit_supported={} appkit_version={:.1} threshold={:.1} os_version={}.{}.{} os_supported={} class_available={}",
            runtime.runtime_supported(),
            runtime.appkit_supported,
            runtime.appkit_version,
            NS_APPKIT_VERSION_26_0,
            runtime.os_version.major_version,
            runtime.os_version.minor_version,
            runtime.os_version.patch_version,
            runtime.os_supported,
            runtime.class_available
        );
        return None;
    }
    let mtm = match MainThreadMarker::new() {
        Some(marker) => marker,
        None => {
            let is_main_thread = if let Some(ns_thread) = Class::get("NSThread") {
                unsafe { msg_send![ns_thread, isMainThread] }
            } else {
                false
            };
            tracing::warn!(
                "NSGlassEffectView fallback to NSVisualEffectView: MainThreadMarker::new() returned None (NSThread.isMainThread={})",
                is_main_thread
            );
            return None;
        }
    };
    let frame = Objc2CGRect::new(
        Objc2CGPoint::new(frame.origin.x, frame.origin.y),
        Objc2CGSize::new(frame.size.width, frame.size.height),
    );
    let view = NSGlassEffectView::initWithFrame(mtm.alloc(), frame);
    view.setStyle(glass_effect_style_for_material(material));
    // Hand the +1 retain to the autorelease pool so the parent's `addSubview:`
    // (which adds its own retain) becomes the sole owner. Without this, the
    // initial alloc/init retain leaked one NSGlassEffectView per call.
    let view: Id = Retained::autorelease_return(view).cast::<Object>();
    unsafe {
        let _: () = msg_send![view, setWantsLayer: true];
        let supports_corner_radius: bool =
            msg_send![view, respondsToSelector: sel!(setCornerRadius:)];
        if supports_corner_radius {
            let _: () = msg_send![view, setCornerRadius: ui_tokens::SURFACE_RADIUS];
        }
    }
    Some(view)
}

#[cfg(not(feature = "liquid_glass"))]
fn create_typed_glass_effect_view(_frame: CGRect, _material: NSVisualEffectMaterial) -> Option<Id> {
    None
}

/// Check whether Tahoe `NSGlassEffectView` is usable on this runtime.
///
/// We intentionally use only official style values:
/// - `Regular` (0)
/// - `Clear` (1)
pub fn glass_effect_supported() -> bool {
    glass_effect_runtime_supported() && glass_effect_view_class_available()
}

// ── Safe NSVisualEffectView subclass ─────────────────────────────────
// macOS 26 Tahoe beta: AppKit internally calls `layoutRegionGuides` on
// NSVisualEffectView during layout, but the method is missing →
// -[NSVisualEffectView layoutRegionGuides]: unrecognized selector.
// We register a thin subclass once that adds a stub returning nil so
// ObjC nil-messaging silently eats any further calls.

static CS_LAYOUT_REGION_GUIDES_INIT: Once = Once::new();

fn ensure_layout_region_guides_for_class(class_name: &str) {
    let Some(cls) = Class::get(class_name) else {
        return;
    };
    let has_method = unsafe { !class_getInstanceMethod(cls, sel!(layoutRegionGuides)).is_null() };
    if has_method {
        return;
    }

    tracing::info!(
        "Injecting layoutRegionGuides stub into {} (Tahoe beta workaround)",
        class_name
    );
    extern "C" fn layout_region_guides(_this: &Object, _cmd: Sel) -> Id {
        std::ptr::null_mut()
    }
    // SAFETY: transmute fn(&Object, Sel) -> Id to Imp (extern "C" fn()).
    // ObjC runtime internally casts Imp to the correct signature via selector dispatch.
    // Encoding "@@:" means: return `id`, args `(id self, SEL _cmd)`, which
    // matches `extern "C" fn(&Object, Sel) -> Id`.
    unsafe {
        let imp: objc::runtime::Imp =
            std::mem::transmute(layout_region_guides as extern "C" fn(&Object, Sel) -> Id);
        let encoding = CString::new("@@:").unwrap();
        objc::runtime::class_addMethod(
            cls as *const Class as *mut Class,
            sel!(layoutRegionGuides),
            imp,
            encoding.as_ptr(),
        );
    }
}

/// Ensure vibrancy classes expose a `layoutRegionGuides` method.
///
/// macOS 26 Tahoe beta: AppKit internally calls `layoutRegionGuides` on
/// NSVisualEffectView during layout, but the method is missing on current betas.
/// A subclass-based fix only protects our instances — AppKit also creates its own
/// NSVisualEffectView internally (e.g. titlebar blur on FullSizeContentView windows).
///
/// This injects the stub method directly into `NSVisualEffectView` itself,
/// protecting ALL instances including AppKit-internal ones. We apply the same
/// fallback for `NSGlassEffectView` when available.
pub fn ensure_layout_region_guides_exists() {
    CS_LAYOUT_REGION_GUIDES_INIT.call_once(|| {
        ensure_layout_region_guides_for_class("NSVisualEffectView");
        ensure_layout_region_guides_for_class("NSGlassEffectView");
    });
}

fn safe_visual_effect_view_class() -> *const Class {
    ensure_layout_region_guides_exists();
    Class::get("NSVisualEffectView").unwrap()
}

/// Create a vibrancy effect view.
///
/// Uses `safe_visual_effect_view_class()` which adds a `layoutRegionGuides`
/// stub on Tahoe 26 beta to prevent the internal AppKit crash.
pub fn create_glass_effect_view(frame: CGRect, material: NSVisualEffectMaterial) -> Id {
    create_glass_effect_view_with(
        frame,
        material,
        NSVisualEffectBlendingMode::WithinWindow,
        NSVisualEffectState::Active,
    )
}

/// Create a vibrancy effect view with explicit blending and state.
pub fn create_glass_effect_view_with(
    frame: CGRect,
    material: NSVisualEffectMaterial,
    blending: NSVisualEffectBlendingMode,
    state: NSVisualEffectState,
) -> Id {
    unsafe {
        if let Some(view) = create_typed_glass_effect_view(frame, material) {
            return view;
        }

        let cls = safe_visual_effect_view_class();
        let view: Id = msg_send![cls, alloc];
        let view: Id = msg_send![view, initWithFrame: frame];
        set_visual_effect_material(view, material);
        set_visual_effect_blending(view, blending);
        set_visual_effect_state(view, state);
        let _: () = msg_send![view, setWantsLayer: true];
        view
    }
}

/// Create an `NSGlassEffectContainerView` when available, otherwise fallback to `NSView`.
pub fn create_glass_effect_container_view(frame: CGRect, spacing: f64) -> Id {
    unsafe {
        let view: Id = if let Some(container_class) = Class::get("NSGlassEffectContainerView") {
            let view: Id = msg_send![container_class, alloc];
            let view: Id = msg_send![view, initWithFrame: frame];
            let supports_spacing: bool = msg_send![view, respondsToSelector: sel!(setSpacing:)];
            if supports_spacing {
                let _: () = msg_send![view, setSpacing: spacing.max(0.0)];
            }
            view
        } else {
            let ns_view = Class::get("NSView").unwrap();
            let view: Id = msg_send![ns_view, alloc];
            msg_send![view, initWithFrame: frame]
        };
        let _: () = msg_send![view, setWantsLayer: true];
        view
    }
}

unsafe fn set_content_view_or_subview(host: Id, content_view: Id) -> bool {
    if host.is_null() || content_view.is_null() {
        return false;
    }
    let supports_content_view: bool =
        unsafe { msg_send![host, respondsToSelector: sel!(setContentView:)] };
    if supports_content_view {
        let _: () = unsafe { msg_send![host, setContentView: content_view] };
        true
    } else {
        let _: () = unsafe { msg_send![host, addSubview: content_view] };
        false
    }
}

/// Attach content to a glass effect view using WWDC25 `contentView` semantics when available.
///
/// Returns `true` when `setContentView:` was used; `false` when it fell back to `addSubview:`.
/// # Safety
/// `glass_view` and `content_view` must be valid Objective-C view objects.
pub unsafe fn set_glass_effect_content_view(glass_view: Id, content_view: Id) -> bool {
    unsafe { set_content_view_or_subview(glass_view, content_view) }
}

/// Attach content to a glass container view using `contentView` when available.
///
/// Returns `true` when `setContentView:` was used; `false` when it fell back to `addSubview:`.
/// # Safety
/// `container_view` and `content_view` must be valid Objective-C view objects.
pub unsafe fn set_glass_container_content_view(container_view: Id, content_view: Id) -> bool {
    unsafe { set_content_view_or_subview(container_view, content_view) }
}

/// # Safety
/// `view` must be a valid `NSVisualEffectView`/`NSGlassEffectView` instance.
pub unsafe fn set_visual_effect_material(view: Id, material: NSVisualEffectMaterial) {
    if view.is_null() {
        return;
    }
    let cls = unsafe { object_getClass(view as *const Object) };
    if cls.is_null() {
        return;
    }
    if unsafe { class_getInstanceMethod(cls, sel!(setMaterial:)) }.is_null() {
        return;
    }
    let _: () = msg_send![view, setMaterial: material];
}

/// # Safety
/// `view` must be a valid `NSVisualEffectView`/`NSGlassEffectView` instance.
pub unsafe fn set_visual_effect_blending(view: Id, blending: NSVisualEffectBlendingMode) {
    if view.is_null() {
        return;
    }
    let cls = unsafe { object_getClass(view as *const Object) };
    if cls.is_null() {
        return;
    }
    if unsafe { class_getInstanceMethod(cls, sel!(setBlendingMode:)) }.is_null() {
        return;
    }
    let _: () = msg_send![view, setBlendingMode: blending];
}

/// # Safety
/// `view` must be a valid `NSVisualEffectView`/`NSGlassEffectView` instance.
pub unsafe fn set_visual_effect_state(view: Id, state: NSVisualEffectState) {
    if view.is_null() {
        return;
    }
    let cls = unsafe { object_getClass(view as *const Object) };
    if cls.is_null() {
        return;
    }
    if unsafe { class_getInstanceMethod(cls, sel!(setState:)) }.is_null() {
        return;
    }
    let _: () = msg_send![view, setState: state];
}

/// Clamp overlay position to visible frame with margin.
pub fn clamp_overlay_position(
    visible_frame: CGRect,
    window_width: f64,
    window_height: f64,
    margin: f64,
    raw_x: f64,
    raw_y: f64,
) -> (f64, f64) {
    let min_x = visible_frame.origin.x + margin;
    let max_x = visible_frame.origin.x + visible_frame.size.width - window_width - margin;
    let min_y = visible_frame.origin.y + margin;
    let max_y = visible_frame.origin.y + visible_frame.size.height - window_height - margin;

    let x = raw_x.clamp(min_x, max_x.max(min_x));
    let y = raw_y.clamp(min_y, max_y.max(min_y));
    (x, y)
}

// ============================================================================
// Text Field Value Helpers
// ============================================================================

/// Set text field string value
/// # Safety
/// `field` must be a valid `NSTextField` (or compatible) instance.
pub unsafe fn set_text(field: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![field, setStringValue: text_str];
    }
}

/// Get text field string value
/// # Safety
/// `field` must be a valid `NSTextField` (or compatible) instance.
pub unsafe fn get_text(field: Id) -> String {
    unsafe {
        let ns_str: Id = msg_send![field, stringValue];
        if ns_str.is_null() {
            return String::new();
        }
        let c_str: *const i8 = msg_send![ns_str, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .into_owned()
    }
}

/// Set text view string (for NSTextView, not NSTextField)
/// # Safety
/// `text_view` must be a valid `NSTextView` instance.
pub unsafe fn set_text_view_string(text_view: Id, text: &str) {
    unsafe {
        let text_str = ns_string(text);
        let _: () = msg_send![text_view, setString: text_str];
    }
}

/// Get text view string (for NSTextView)
/// # Safety
/// `text_view` must be a valid `NSTextView` instance.
pub unsafe fn get_text_view_string(text_view: Id) -> String {
    unsafe {
        let ns_str: Id = msg_send![text_view, string];
        if ns_str.is_null() {
            return String::new();
        }
        let c_str: *const i8 = msg_send![ns_str, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .into_owned()
    }
}

// ============================================================================
// Animation Helpers
// ============================================================================

/// Run a simple fade animation
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn animate_fade(window: Id, to_alpha: f64, duration: f64) {
    unsafe {
        let ns_animation_context = Class::get("NSAnimationContext").unwrap();

        let _: () = msg_send![ns_animation_context, beginGrouping];
        let ctx: Id = msg_send![ns_animation_context, currentContext];
        let _: () = msg_send![ctx, setDuration: duration];

        let animator: Id = msg_send![window, animator];
        let _: () = msg_send![animator, setAlphaValue: to_alpha];

        let _: () = msg_send![ns_animation_context, endGrouping];
    }
}

/// Animate window width change (horizontal slide for drawer collapse)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn animate_window_width(window: Id, to_width: f64, duration: f64) {
    unsafe {
        let ns_animation_context = Class::get("NSAnimationContext").unwrap();

        // Get current frame
        let current_frame: CGRect = msg_send![window, frame];

        // Calculate new frame with same origin and height but new width
        let new_frame = CGRect::new(
            &current_frame.origin,
            &CGSize::new(to_width, current_frame.size.height),
        );

        let _: () = msg_send![ns_animation_context, beginGrouping];
        let ctx: Id = msg_send![ns_animation_context, currentContext];
        let _: () = msg_send![ctx, setDuration: duration];

        // Animate frame change
        let animator: Id = msg_send![window, animator];
        let _: () = msg_send![animator, setFrame: new_frame display: true];

        let _: () = msg_send![ns_animation_context, endGrouping];
    }
}

// ============================================================================
// View Visibility Helpers
// ============================================================================

/// Set view hidden state
/// # Safety
/// `view` must be a valid `NSView` (or subclass) instance.
pub unsafe fn set_hidden(view: Id, hidden: bool) {
    unsafe {
        let _: () = msg_send![view, setHidden: hidden];
    }
}

/// Set view enabled state (for buttons)
/// # Safety
/// `view` must be a valid `NSView`/`NSControl` instance.
pub unsafe fn set_enabled(view: Id, enabled: bool) {
    unsafe {
        let _: () = msg_send![view, setEnabled: enabled];
    }
}

/// Opt into a visible focus ring for keyboard navigation.
/// # Safety
/// `view` must be a valid NSView instance.
pub unsafe fn set_focus_ring(view: Id) {
    unsafe {
        let _: () = msg_send![view, setFocusRingType: NS_FOCUS_RING_TYPE_EXTERIOR];
    }
}

/// Return a monospaced system font (best-effort).
/// # Safety
/// Uses AppKit selectors; caller must be on main thread when applied to views.
pub unsafe fn monospace_font(size: f64) -> Id {
    unsafe {
        let ns_font = Class::get("NSFont").unwrap();
        let supports: bool =
            msg_send![ns_font, respondsToSelector: sel!(monospacedSystemFontOfSize:weight:)];
        if supports {
            let font: Id = msg_send![ns_font, monospacedSystemFontOfSize: size weight: 0.0];
            if !font.is_null() {
                return font;
            }
        }

        let font: Id = msg_send![ns_font, userFixedPitchFontOfSize: size];
        if !font.is_null() {
            font
        } else {
            msg_send![ns_font, systemFontOfSize: size]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::geometry::CGPoint;
    use serial_test::serial;

    #[test]
    fn markdown_table_detection_handles_common_patterns() {
        let table = "| Name | Value |\n| ---- | ----- |\n| A | 1 |";
        assert!(looks_like_markdown_table(table));

        let plain = "line one\nline two\nline three";
        assert!(!looks_like_markdown_table(plain));
    }

    #[test]
    fn native_markdown_is_bypassed_for_chat_bubbles() {
        let table = "# Report\n\n| Name | Value |\n| ---- | ----- |\n| A | 1 |";
        assert!(!should_apply_native_markdown(table));

        let inline_markdown = "**bold** `code`";
        assert!(!should_apply_native_markdown(inline_markdown));
    }

    #[test]
    fn chat_header_layout_avoids_cluster_collisions() {
        let header_w = 450.0;
        let right_pad = ui_tokens::EDGE_PADDING_TIGHT;
        let cluster_w =
            ui_tokens::CHAT_HEADER_BUTTON_SIZE * 5.0 + ui_tokens::CHAT_HEADER_BUTTON_GAP * 4.0;
        let right_cluster_start_x = header_w - right_pad - cluster_w;
        let title_x = ui_tokens::EDGE_PADDING_TIGHT;
        let title_w = ui_tokens::CHAT_TITLE_LABEL_WIDTH;

        let layout = chat_header_layout(title_x, title_w, right_cluster_start_x);
        let tabs_right =
            layout.tab_cluster_x + layout.tab_button_width * 3.0 + layout.tab_button_gap * 2.0;
        assert!(tabs_right <= right_cluster_start_x - ui_tokens::CHAT_HEADER_GROUP_GAP + 0.001);
        if layout.show_status_pill {
            assert!(layout.status_pill_x >= tabs_right + ui_tokens::CHAT_HEADER_GROUP_GAP - 0.001);
            assert!(
                layout.status_pill_x + layout.status_pill_width
                    <= right_cluster_start_x - ui_tokens::CHAT_HEADER_GROUP_GAP + 0.001
            );
        }
    }

    #[test]
    fn chat_header_layout_hides_status_when_space_is_tight() {
        let layout = chat_header_layout(12.0, ui_tokens::CHAT_TITLE_LABEL_WIDTH, 142.0);
        assert!(!layout.show_status_pill);
        let tabs_right =
            layout.tab_cluster_x + layout.tab_button_width * 3.0 + layout.tab_button_gap * 2.0;
        assert!(tabs_right <= 142.0 - ui_tokens::CHAT_HEADER_GROUP_GAP + 0.001);
    }

    #[test]
    fn chat_header_layout_keeps_status_before_right_cluster() {
        let right_cluster_start_x = 270.0;
        let layout = chat_header_layout(
            86.0,
            ui_tokens::CHAT_TITLE_LABEL_WIDTH,
            right_cluster_start_x,
        );
        if layout.show_status_pill {
            let right_anchor = right_cluster_start_x - ui_tokens::CHAT_HEADER_GROUP_GAP;
            assert!(layout.status_pill_x + layout.status_pill_width <= right_anchor + 0.001);
        }
    }

    #[test]
    fn chat_input_row_layout_keeps_buttons_on_sides() {
        let layout = chat_input_row_layout(420.0, ui_tokens::AGENT_INPUT_HEIGHT);
        assert!(
            layout.attach_x + layout.button_width + ui_tokens::CHAT_INPUT_CONTROL_GAP
                <= layout.text_x
        );
        assert!(
            layout.text_x + layout.text_width + ui_tokens::CHAT_INPUT_CONTROL_GAP <= layout.send_x
        );
    }

    #[test]
    fn chat_input_row_layout_avoids_overlap_on_narrow_width() {
        let layout = chat_input_row_layout(140.0, ui_tokens::AGENT_INPUT_HEIGHT);
        assert!(layout.text_width >= 0.0);
        assert!(layout.attach_x + layout.button_width <= layout.send_x);
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn layout_insets_default_are_non_negative() {
        if std::env::var("CODESCRIBE_UI_TESTS").is_err() {
            return;
        }
        unsafe {
            let ns_view = Class::get("NSView").unwrap();
            let view: Id = msg_send![ns_view, alloc];
            let view: Id = msg_send![
                view,
                initWithFrame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(120.0, 80.0))
            ];
            let insets = layout_insets_for_view(view);
            assert!(insets.left >= 0.0);
            assert!(insets.right >= 0.0);
            assert!(insets.top >= 0.0);
            assert!(insets.bottom >= 0.0);
        }
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn markdown_render_applies_or_falls_back() {
        if std::env::var("CODESCRIBE_UI_TESTS").is_err() {
            return;
        }
        unsafe {
            let ns_text = Class::get("NSTextField").unwrap();
            let ns_font = Class::get("NSFont").unwrap();
            let field: Id = msg_send![ns_text, alloc];
            let field: Id = msg_send![
                field,
                initWithFrame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(200.0, 60.0))
            ];
            let font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
            let applied = apply_markdown_to_text_field(field, "**bold** `code`", font);
            let text = get_text(field);
            assert!(text.contains("bold"));
            assert!(text.contains("code"));
            if applied {
                let attr: Id = msg_send![field, attributedStringValue];
                let len: usize = msg_send![attr, length];
                assert!(len >= text.len());
            }
        }
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn set_button_symbol_uses_sf_symbols() {
        if std::env::var("CODESCRIBE_UI_TESTS").is_err() {
            return;
        }
        unsafe {
            let ns_button = Class::get("NSButton").unwrap();
            let button: Id = msg_send![ns_button, alloc];
            let button: Id = msg_send![
                button,
                initWithFrame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(24.0, 24.0))
            ];
            assert!(set_button_symbol(button, "tray.full"));
        }
    }
}
