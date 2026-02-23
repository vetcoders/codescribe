//! Native AppKit UI helpers for CodeScribe
//!
//! Reduces msg_send! boilerplate by providing high-level functions for common UI patterns.
//! These helpers wrap Objective-C calls in safe, reusable Rust functions.
//!
//! # Safety
//! All functions in this module operate on raw Objective-C pointers (`Id = *mut Object`).
//! Callers must ensure pointers are valid. This is standard for Rust-ObjC FFI.
//!
//! Created by M&K (c)2026 VetCoders

use crate::os::clipboard;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::declare::ClassDecl;
use objc::runtime::Sel;
use objc::runtime::{Class, Object, class_getInstanceMethod, object_getClass};
use objc::{msg_send, sel, sel_impl};
#[cfg(feature = "liquid_glass")]
use objc2::MainThreadMarker;
#[cfg(feature = "liquid_glass")]
use objc2::rc::Retained;
#[cfg(feature = "liquid_glass")]
use objc2_app_kit::{NSAppKitVersionNumber, NSGlassEffectView, NSGlassEffectViewStyle};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
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

    // ─── Tafla: unified surface design system ──────────────────────
    // Glass = frame (cool, system materials).  Paper = content (warm, readable).
    // One radius, one border, no stacking — flat pane, not bubble soup.

    /// Canonical corner radius — use this everywhere instead of LG/MD/SM mix.
    pub const SURFACE_RADIUS: f64 = 12.0;
    pub const SURFACE_BORDER_WIDTH: f64 = 1.0;
    pub const SURFACE_BORDER_ALPHA: f64 = 0.14;

    /// Glass background: alpha for vibrancy-backed views.
    pub const GLASS_BG_ALPHA: f64 = 0.20;
    /// Glass fallback: alpha when NSVisualEffectView is not available.
    pub const GLASS_FALLBACK_ALPHA: f64 = 0.30;

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

    /// Dictation overlay tuning: lighter sheet + compact action row.
    pub const OVERLAY_GLASS_BG_ALPHA: f64 = 0.16;
    pub const OVERLAY_GLASS_FALLBACK_ALPHA: f64 = 0.24;
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
        control_bg_tint(adaptive_alpha(0.27, 0.38))
    }

    pub fn panel_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.35, 0.46))
    }

    pub fn settings_glass_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.30, 0.40))
    }

    pub fn input_bar_bg() -> Id {
        control_bg_tint(adaptive_alpha(0.24, 0.34))
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

#[derive(Debug, Clone, Copy)]
pub struct ChatHeaderLayout {
    pub tab_cluster_x: f64,
    pub tab_button_width: f64,
    pub tab_button_gap: f64,
    pub status_pill_x: f64,
    pub status_pill_width: f64,
    pub show_status_pill: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ChatInputRowLayout {
    pub attach_x: f64,
    pub attach_y: f64,
    pub send_x: f64,
    pub send_y: f64,
    pub button_width: f64,
    pub button_height: f64,
    pub text_x: f64,
    pub text_y: f64,
    pub text_width: f64,
    pub text_height: f64,
}

pub fn chat_header_layout(
    title_x: f64,
    title_width: f64,
    right_cluster_start_x: f64,
) -> ChatHeaderLayout {
    use ui_tokens::{
        CHAT_HEADER_BUTTON_SIZE, CHAT_HEADER_GROUP_GAP, CHAT_TAB_BUTTON_COLLAPSED_WIDTH,
        CHAT_TAB_BUTTON_GAP, CHAT_TAB_BUTTON_MIN_GAP, CHAT_TAB_BUTTON_MIN_WIDTH,
        STATUS_PILL_MIN_WIDTH, STATUS_PILL_WIDTH,
    };

    let left_anchor = (title_x + title_width + CHAT_HEADER_GROUP_GAP).max(0.0);
    let right_anchor = (right_cluster_start_x - CHAT_HEADER_GROUP_GAP).max(left_anchor);
    let available = (right_anchor - left_anchor).max(0.0);

    let tab_target_width = (CHAT_HEADER_BUTTON_SIZE - 2.0).max(CHAT_TAB_BUTTON_MIN_WIDTH);
    let tab_target_gap = CHAT_TAB_BUTTON_GAP.max(CHAT_TAB_BUTTON_MIN_GAP);
    let min_tab_total = CHAT_TAB_BUTTON_MIN_WIDTH * 3.0 + CHAT_TAB_BUTTON_MIN_GAP * 2.0;

    let mut show_status =
        available >= (min_tab_total + CHAT_HEADER_GROUP_GAP + STATUS_PILL_MIN_WIDTH);
    let mut status_width = if show_status {
        (available - CHAT_HEADER_GROUP_GAP - min_tab_total)
            .clamp(STATUS_PILL_MIN_WIDTH, STATUS_PILL_WIDTH)
    } else {
        0.0
    };

    let mut tab_space = if show_status {
        (available - CHAT_HEADER_GROUP_GAP - status_width).max(0.0)
    } else {
        available
    };

    let mut tab_gap = tab_target_gap;
    let mut tab_width = ((tab_space - tab_gap * 2.0) / 3.0).min(tab_target_width);

    if tab_width < CHAT_TAB_BUTTON_MIN_WIDTH {
        tab_width = CHAT_TAB_BUTTON_MIN_WIDTH;
        tab_gap =
            ((tab_space - tab_width * 3.0) / 2.0).clamp(CHAT_TAB_BUTTON_MIN_GAP, tab_target_gap);
    }

    let min_gap_fit_width = CHAT_TAB_BUTTON_MIN_GAP * 2.0 + CHAT_TAB_BUTTON_COLLAPSED_WIDTH * 3.0;
    if tab_space < min_gap_fit_width {
        show_status = false;
        status_width = 0.0;
        tab_space = available;
        tab_gap = CHAT_TAB_BUTTON_MIN_GAP;
        tab_width = ((tab_space - tab_gap * 2.0) / 3.0).min(tab_target_width);
    }

    tab_space = tab_space.max(0.0);
    let max_gap = (tab_space / 2.0).max(0.0);
    tab_gap = tab_gap.min(max_gap);
    let max_width_for_space = ((tab_space - tab_gap * 2.0) / 3.0).max(0.0);
    tab_width = tab_width.min(max_width_for_space);

    let mut tab_total = (tab_width * 3.0 + tab_gap * 2.0).max(0.0);
    if show_status {
        let min_status_x = left_anchor + tab_total + CHAT_HEADER_GROUP_GAP;
        let max_status_width = (right_anchor - min_status_x).max(0.0);
        if max_status_width < STATUS_PILL_MIN_WIDTH {
            show_status = false;
            status_width = 0.0;
            tab_space = available.max(0.0);
            tab_gap = tab_target_gap.min((tab_space / 2.0).max(0.0));
            tab_width = ((tab_space - tab_gap * 2.0) / 3.0)
                .max(0.0)
                .min(tab_target_width);
            tab_total = (tab_width * 3.0 + tab_gap * 2.0).max(0.0);
        } else {
            status_width = status_width.min(max_status_width);
        }
    }
    let status_x = if show_status {
        (right_anchor - status_width).max(left_anchor + tab_total + CHAT_HEADER_GROUP_GAP)
    } else {
        right_anchor
    };

    ChatHeaderLayout {
        tab_cluster_x: left_anchor,
        tab_button_width: tab_width.max(0.0),
        tab_button_gap: tab_gap.max(0.0),
        status_pill_x: status_x,
        status_pill_width: status_width,
        show_status_pill: show_status,
    }
}

pub fn chat_input_row_layout(bar_width: f64, bar_height: f64) -> ChatInputRowLayout {
    use ui_tokens::{
        CHAT_INPUT_BUTTON_HEIGHT, CHAT_INPUT_BUTTON_WIDTH, CHAT_INPUT_CONTROL_GAP,
        CHAT_INPUT_SIDE_INSET, CHAT_INPUT_TEXT_INSET_Y,
    };

    let button_width = CHAT_INPUT_BUTTON_WIDTH;
    let button_height = CHAT_INPUT_BUTTON_HEIGHT;
    let side_inset = CHAT_INPUT_SIDE_INSET.max(0.0);
    let control_gap = CHAT_INPUT_CONTROL_GAP.max(0.0);

    let attach_x = side_inset;
    let send_x = (bar_width - side_inset - button_width).max(attach_x + button_width + control_gap);
    let text_x = attach_x + button_width + control_gap;
    let text_right = (send_x - control_gap).max(text_x);
    let text_width = (text_right - text_x).max(0.0);

    let button_y = ((bar_height - button_height) * 0.5).max(6.0);
    let text_y = CHAT_INPUT_TEXT_INSET_Y.max(0.0);
    let text_height = (bar_height - text_y * 2.0).max(24.0);

    ChatInputRowLayout {
        attach_x,
        attach_y: button_y,
        send_x,
        send_y: button_y,
        button_width,
        button_height,
        text_x,
        text_y,
        text_width,
        text_height,
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
        // Keep side panes and title-like strips lighter.
        NSVisualEffectMaterial::Sidebar | NSVisualEffectMaterial::Titlebar => {
            NSGlassEffectViewStyle::Clear
        }
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
    let view: Id = Retained::into_raw(view).cast::<Object>();
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
    #[allow(clippy::transmute_ptr_to_ptr)]
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

// ============================================================================
// String Helpers
// ============================================================================

/// Create an NSString from a Rust &str
pub fn ns_string(s: &str) -> Id {
    unsafe {
        let ns_string = Class::get("NSString").unwrap();
        let c_str = CString::new(s).unwrap_or_else(|_| CString::new("").unwrap());
        msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()]
    }
}

/// Copy text to the system clipboard (best-effort).
pub fn copy_to_clipboard(text: &str) {
    let _ = clipboard::copy(text);
}

// ============================================================================
// Text Field Helpers
// ============================================================================

/// Get string value from an NSTextField/NSSearchField
/// # Safety
/// `field` must be a valid `NSTextField`/`NSSearchField` instance.
pub unsafe fn get_text_field_string(field: Id) -> String {
    unsafe {
        let value: Id = msg_send![field, stringValue];
        let c_str: *const i8 = msg_send![value, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string()
    }
}

/// Set string value for an NSTextField/NSSearchField
/// # Safety
/// `field` must be a valid `NSTextField`/`NSSearchField` instance.
pub unsafe fn set_text_field_string(field: Id, text: &str) {
    unsafe {
        let value = ns_string(text);
        let _: () = msg_send![field, setStringValue: value];
    }
}

/// Set tooltip for a control/view.
/// # Safety
/// `view` must be a valid Objective-C object that supports `setToolTip:`.
pub unsafe fn set_tooltip(view: Id, text: &str) {
    unsafe {
        let tip = ns_string(text);
        let _: () = msg_send![view, setToolTip: tip];
    }
}

// ============================================================================
// Label / TextField Helpers
// ============================================================================

/// Configuration for creating a label
pub struct LabelConfig {
    pub frame: CGRect,
    pub text: String,
    pub font_size: f64,
    pub bold: bool,
    pub text_color: Id,
    pub background_color: Option<Id>,
    pub selectable: bool,
    pub editable: bool,
}

impl Default for LabelConfig {
    fn default() -> Self {
        Self {
            frame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(100.0, 24.0)),
            text: String::new(),
            font_size: 13.0,
            bold: false,
            text_color: color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        }
    }
}

/// Create a label (non-editable text field)
pub fn create_label(config: LabelConfig) -> Id {
    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_text_field, alloc];
        let field: Id = msg_send![field, initWithFrame: config.frame];

        let _: () = msg_send![field, setBezeled: false];
        let _: () = msg_send![field, setEditable: config.editable];
        let _: () = msg_send![field, setSelectable: config.selectable];

        if let Some(bg) = config.background_color {
            let _: () = msg_send![field, setDrawsBackground: true];
            let _: () = msg_send![field, setBackgroundColor: bg];
        } else {
            let _: () = msg_send![field, setDrawsBackground: false];
        }

        let _: () = msg_send![field, setTextColor: config.text_color];

        let font: Id = if config.bold {
            msg_send![ns_font, boldSystemFontOfSize: config.font_size]
        } else {
            msg_send![ns_font, systemFontOfSize: config.font_size]
        };
        let _: () = msg_send![field, setFont: font];

        let text = ns_string(&config.text);
        let _: () = msg_send![field, setStringValue: text];

        field
    }
}

/// Create a search field
pub fn create_search_field(frame: CGRect, placeholder: &str) -> Id {
    unsafe {
        let ns_search_field = Class::get("NSSearchField").unwrap();
        let field: Id = msg_send![ns_search_field, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];
        let placeholder = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: placeholder];
        field
    }
}

/// Quick label creation with minimal config
pub fn label(frame: CGRect, text: &str) -> Id {
    create_label(LabelConfig {
        frame,
        text: text.to_string(),
        ..Default::default()
    })
}

/// Quick label with custom font size
pub fn label_sized(frame: CGRect, text: &str, font_size: f64, bold: bool) -> Id {
    create_label(LabelConfig {
        frame,
        text: text.to_string(),
        font_size,
        bold,
        ..Default::default()
    })
}

// ============================================================================
// Segmented Control Helpers
// ============================================================================

/// Create a segmented control with labels
pub fn create_segmented_control(frame: CGRect, labels: &[&str]) -> Id {
    unsafe {
        let ns_segmented = Class::get("NSSegmentedControl").unwrap();
        let control: Id = msg_send![ns_segmented, alloc];
        let control: Id = msg_send![control, initWithFrame: frame];
        let _: () = msg_send![control, setSegmentCount: labels.len() as isize];
        for (idx, label) in labels.iter().enumerate() {
            let title = ns_string(label);
            let _: () = msg_send![control, setLabel: title forSegment: idx as isize];
        }
        let _: () = msg_send![control, setSelectedSegment: 0_isize];
        control
    }
}

// ============================================================================
// Button Helpers
// ============================================================================

/// Button style constants
pub mod button_style {
    pub const ROUNDED: isize = 1;
    pub const REGULAR_SQUARE: isize = 2;
    pub const SHADOWLESS_SQUARE: isize = 6;
    pub const SMALL_SQUARE: isize = 10;
    pub const INLINE: isize = 15;
    pub const GLASS: isize = 16;
}

/// Create a button with title and action
pub fn create_button(frame: CGRect, title: &str, style: isize) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();

        let btn: Id = msg_send![ns_button, alloc];
        let btn: Id = msg_send![btn, initWithFrame: frame];

        let title_str = ns_string(title);
        let _: () = msg_send![btn, setTitle: title_str];
        let _: () = msg_send![btn, setBezelStyle: style];

        btn
    }
}

/// Set a button's SF Symbol image (returns true if applied).
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn set_button_symbol(button: Id, symbol_name: &str) -> bool {
    unsafe {
        let Some(ns_image) = Class::get("NSImage") else {
            return false;
        };
        let responds: bool = msg_send![
            ns_image,
            respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)
        ];
        if !responds {
            return false;
        }
        let name = ns_string(symbol_name);
        let desc = ns_string("");
        let image: Id = msg_send![
            ns_image,
            imageWithSystemSymbolName: name
            accessibilityDescription: desc
        ];
        if image.is_null() {
            return false;
        }
        let _: () = msg_send![button, setImage: image];
        // NSImageOnly == 1
        let _: () = msg_send![button, setImagePosition: 1_isize];
        true
    }
}

/// Set an SF Symbol image on a segmented control segment.
/// # Safety
/// `control` must be a valid NSSegmentedControl instance.
pub unsafe fn set_segment_symbol(control: Id, segment: isize, symbol_name: &str) -> bool {
    let Some(ns_image) = Class::get("NSImage") else {
        return false;
    };
    let responds: bool = msg_send![
        ns_image,
        respondsToSelector: sel!(imageWithSystemSymbolName:accessibilityDescription:)
    ];
    if !responds {
        return false;
    }
    let name = ns_string(symbol_name);
    let desc = ns_string("");
    let image: Id = msg_send![
        ns_image,
        imageWithSystemSymbolName: name
        accessibilityDescription: desc
    ];
    if image.is_null() {
        return false;
    }
    let _: () = msg_send![control, setImage: image forSegment: segment];
    true
}

/// Style a toolbar icon button (borderless, inline bezel, tinted symbol).
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn style_toolbar_icon_button(button: Id) {
    let _: () = msg_send![button, setBezelStyle: button_style::INLINE];
    let responds_bordered: bool = msg_send![button, respondsToSelector: sel!(setBordered:)];
    if responds_bordered {
        let _: () = msg_send![button, setBordered: false];
    }
    // NOTE: Do NOT set transparent=true — it hides the button image entirely.
    // setBordered=false + setImagePosition=NSImageOnly is sufficient for borderless icons.
    let responds_shows_border: bool =
        msg_send![button, respondsToSelector: sel!(setShowsBorderOnlyWhileMouseInside:)];
    if responds_shows_border {
        let _: () = msg_send![button, setShowsBorderOnlyWhileMouseInside: false];
    }
    let responds_image_position: bool =
        msg_send![button, respondsToSelector: sel!(setImagePosition:)];
    if responds_image_position {
        // NSImageOnly == 1
        let _: () = msg_send![button, setImagePosition: 1_isize];
    }
    let responds_tint: bool = msg_send![button, respondsToSelector: sel!(setContentTintColor:)];
    if responds_tint {
        let tint = color_label();
        let _: () = msg_send![button, setContentTintColor: tint];
    }
    let responds_control_size: bool = msg_send![button, respondsToSelector: sel!(setControlSize:)];
    if responds_control_size {
        let _: () = msg_send![button, setControlSize: 1_isize]; // NSSmallControlSize
    }
}

/// Update a toolbar icon button to reflect active/inactive tab state.
/// # Safety
/// `button` must be a valid NSButton instance.
pub unsafe fn set_tab_button_active(button: Id, active: bool) {
    let ns_color = Class::get("NSColor").unwrap();
    let responds_tint: bool = msg_send![button, respondsToSelector: sel!(setContentTintColor:)];
    if responds_tint {
        let tint: Id = if active {
            msg_send![ns_color, controlAccentColor]
        } else {
            msg_send![ns_color, secondaryLabelColor]
        };
        let _: () = msg_send![button, setContentTintColor: tint];
    }
    let responds_state: bool = msg_send![button, respondsToSelector: sel!(setState:)];
    if responds_state {
        let _: () = msg_send![button, setState: 0_isize];
    }
}

/// Set button target and action
/// # Safety
/// `button` and `target` must be valid Objective-C objects.
pub unsafe fn button_set_action(button: Id, target: Id, action: objc::runtime::Sel) {
    unsafe {
        let _: () = msg_send![button, setTarget: target];
        let _: () = msg_send![button, setAction: action];
    }
}

/// Quick rounded button
pub fn button(frame: CGRect, title: &str) -> Id {
    create_button(frame, title, button_style::ROUNDED)
}

// ============================================================================
// Toggle Helpers
// ============================================================================

/// Create a native NSSwitch toggle.
pub fn create_toggle(frame: CGRect, checked: bool) -> Id {
    unsafe {
        let ns_switch = Class::get("NSSwitch").unwrap();
        let toggle: Id = msg_send![ns_switch, alloc];
        let toggle: Id = msg_send![toggle, initWithFrame: frame];
        let state: isize = if checked { 1 } else { 0 };
        let _: () = msg_send![toggle, setState: state];
        toggle
    }
}

// ============================================================================
// Card Helpers
// ============================================================================

/// Create a drawer card view with a title, subtitle, and preview text
pub fn create_card_view(frame: CGRect, title: &str, subtitle: &str, preview: &str) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let card: Id = msg_send![ns_view, alloc];
        let card: Id = msg_send![card, initWithFrame: frame];
        let _: () = msg_send![card, setWantsLayer: true];
        let layer: Id = msg_send![card, layer];
        if !layer.is_null() {
            let bg_color: Id = ui_colors::card_bg();
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            apply_tafla_surface(layer, true);
            // Drawer cards stay intentionally flat: one border, no halo.
            let _: () = msg_send![layer, setMasksToBounds: true];
            let _: () = msg_send![layer, setShadowRadius: 0.0f64];
            let _: () = msg_send![layer, setShadowOffset: CGSize::new(0.0, 0.0)];
        }

        let title_frame = CGRect::new(
            &CGPoint::new(12.0, frame.size.height - 24.0),
            &CGSize::new(frame.size.width - 24.0, 18.0),
        );
        let title_field: Id = msg_send![ns_text_field, alloc];
        let title_field: Id = msg_send![title_field, initWithFrame: title_frame];
        let _: () = msg_send![title_field, setBezeled: false];
        let _: () = msg_send![title_field, setDrawsBackground: false];
        let _: () = msg_send![title_field, setEditable: false];
        let _: () = msg_send![title_field, setSelectable: false];
        let title_text = ns_string(title);
        let _: () = msg_send![title_field, setStringValue: title_text];
        let title_font: Id = msg_send![ns_font, boldSystemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![title_field, setFont: title_font];
        let title_color: Id = color_label();
        let _: () = msg_send![title_field, setTextColor: title_color];
        let _: () = msg_send![card, addSubview: title_field];

        let subtitle_frame = CGRect::new(
            &CGPoint::new(12.0, frame.size.height - 42.0),
            &CGSize::new(frame.size.width - 24.0, 16.0),
        );
        let subtitle_field: Id = msg_send![ns_text_field, alloc];
        let subtitle_field: Id = msg_send![subtitle_field, initWithFrame: subtitle_frame];
        let _: () = msg_send![subtitle_field, setBezeled: false];
        let _: () = msg_send![subtitle_field, setDrawsBackground: false];
        let _: () = msg_send![subtitle_field, setEditable: false];
        let _: () = msg_send![subtitle_field, setSelectable: false];
        let subtitle_text = ns_string(subtitle);
        let _: () = msg_send![subtitle_field, setStringValue: subtitle_text];
        let subtitle_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::SMALL_FONT_SIZE];
        let _: () = msg_send![subtitle_field, setFont: subtitle_font];
        let subtitle_color: Id = color_secondary_label();
        let _: () = msg_send![subtitle_field, setTextColor: subtitle_color];
        let _: () = msg_send![card, addSubview: subtitle_field];

        // Leave room for the actions row ("Copy / Edit / Delete / ♥") at the bottom.
        let preview_bottom = 36.0;
        let preview_top = 56.0;
        let preview_frame = CGRect::new(
            &CGPoint::new(12.0, preview_bottom),
            &CGSize::new(
                frame.size.width - 24.0,
                (frame.size.height - preview_top - preview_bottom).max(18.0),
            ),
        );
        let preview_field: Id = msg_send![ns_text_field, alloc];
        let preview_field: Id = msg_send![preview_field, initWithFrame: preview_frame];
        let _: () = msg_send![preview_field, setBezeled: false];
        let _: () = msg_send![preview_field, setDrawsBackground: false];
        let _: () = msg_send![preview_field, setEditable: false];
        let _: () = msg_send![preview_field, setSelectable: false];
        let _: () = msg_send![preview_field, setLineBreakMode: 0];
        let preview_text = ns_string(preview);
        let _: () = msg_send![preview_field, setStringValue: preview_text];
        let preview_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::BODY_FONT_SIZE];
        let _: () = msg_send![preview_field, setFont: preview_font];
        let preview_color: Id = color_secondary_label();
        let _: () = msg_send![preview_field, setTextColor: preview_color];
        let _: () = msg_send![card, addSubview: preview_field];

        card
    }
}

// ============================================================================
// Scroll View + Text View Helpers
// ============================================================================

/// Create a scroll view with embedded text view for multi-line text display
pub fn create_scrollable_text_view(frame: CGRect, editable: bool) -> (Id, Id) {
    unsafe {
        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let ns_text_view = Class::get("NSTextView").unwrap();
        let ns_color = Class::get("NSColor").unwrap();

        // Create scroll view
        let scroll: Id = msg_send![ns_scroll_view, alloc];
        let scroll: Id = msg_send![scroll, initWithFrame: frame];
        // Keep scrolling enabled; hide scrollbars via overlay + autohide.
        let _: () = msg_send![scroll, setHasVerticalScroller: true];
        let _: () = msg_send![scroll, setHasHorizontalScroller: false];
        let _: () = msg_send![scroll, setDrawsBackground: false];
        let _: () = msg_send![scroll, setBorderType: 0_isize]; // NSNoBorder
        let _: () = msg_send![scroll, setAutohidesScrollers: true];
        // NSScrollerStyleOverlay == 1
        let _: () = msg_send![scroll, setScrollerStyle: 1_isize];

        // Create text view with same size
        let text_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(frame.size.width, frame.size.height),
        );
        let text_view: Id = msg_send![ns_text_view, alloc];
        let text_view: Id = msg_send![text_view, initWithFrame: text_frame];

        let _: () = msg_send![text_view, setEditable: editable];
        let _: () = msg_send![text_view, setSelectable: true];
        if editable {
            let responds_placeholder: bool =
                msg_send![text_view, respondsToSelector: sel!(setPlaceholderString:)];
            if responds_placeholder {
                let placeholder = ns_string("Type a message");
                let _: () = msg_send![text_view, setPlaceholderString: placeholder];
            }
        }

        // Transparent background
        let clear: Id = msg_send![ns_color, clearColor];
        let _: () = msg_send![text_view, setBackgroundColor: clear];

        // Dynamic text color (light/dark).
        let text_color: Id = msg_send![ns_color, textColor];
        let _: () = msg_send![text_view, setTextColor: text_color];
        let responds_caret: bool =
            msg_send![text_view, respondsToSelector: sel!(setInsertionPointColor:)];
        if responds_caret {
            let caret: Id = msg_send![ns_color, controlAccentColor];
            let _: () = msg_send![text_view, setInsertionPointColor: caret];
        }

        // Auto-resize with scroll view
        let _: () = msg_send![text_view, setMinSize: CGSize::new(0.0, frame.size.height)];
        let _: () = msg_send![text_view, setMaxSize: CGSize::new(f64::MAX, f64::MAX)];
        let _: () = msg_send![text_view, setVerticallyResizable: true];
        let _: () = msg_send![text_view, setHorizontallyResizable: false];

        // Text container settings
        let container: Id = msg_send![text_view, textContainer];
        let _: () = msg_send![container, setWidthTracksTextView: true];

        // Set as document view
        let _: () = msg_send![scroll, setDocumentView: text_view];

        (scroll, text_view)
    }
}

// ============================================================================
// Window Helpers
// ============================================================================

/// Create a floating overlay window
pub fn create_floating_window(
    frame: CGRect,
    title: &str,
    transparent_titlebar: bool,
    resizable: bool,
) -> Id {
    unsafe {
        let ns_window = Class::get("NSWindow").unwrap();

        let mut style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        if transparent_titlebar {
            style |= NSWindowStyleMask::FullSizeContentView;
        }
        if resizable {
            style |= NSWindowStyleMask::Resizable;
        }

        let window: Id = msg_send![ns_window, alloc];
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style
            backing: NSBackingStoreType::Buffered
            defer: false
        ];

        if transparent_titlebar {
            // Re-apply FullSizeContentView post-init to avoid AppKit falling back to
            // separate titlebar/content regions (which visually looks like a duplicate top bar).
            let current_style: NSWindowStyleMask = msg_send![window, styleMask];
            let full_style = current_style | NSWindowStyleMask::FullSizeContentView;
            let _: () = msg_send![window, setStyleMask: full_style];
            let _: () = msg_send![window, setTitleVisibility: 1_isize]; // NSWindowTitleHidden
            let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
            let _: () = msg_send![window, setOpaque: false];
            let ns_color = Class::get("NSColor").unwrap();
            let clear: Id = msg_send![ns_color, clearColor];
            let _: () = msg_send![window, setBackgroundColor: clear];
        }

        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        // Keep the window instance alive even after close; we manage lifecycle explicitly.
        let _: () = msg_send![window, setReleasedWhenClosed: false];

        // Can join all spaces
        let collection = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection];

        if !title.is_empty() {
            let title_str = ns_string(title);
            let _: () = msg_send![window, setTitle: title_str];
        }

        window
    }
}

/// Get window's content view
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

/// Set window alpha (for fade animations)
/// # Safety
/// `window` must be a valid `NSWindow` instance.
pub unsafe fn window_set_alpha(window: Id, alpha: f64) {
    unsafe {
        let _: () = msg_send![window, setAlphaValue: alpha];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn markdown_table_detection_handles_common_patterns() {
        let table = "| Name | Value |\n| ---- | ----- |\n| A | 1 |";
        assert!(looks_like_markdown_table(table));

        let plain = "line one\nline two\nline three";
        assert!(!looks_like_markdown_table(plain));
    }

    #[test]
    fn clamp_overlay_position_keeps_window_inside_frame() {
        let visible = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(100.0, 100.0));
        let (x, y) = clamp_overlay_position(visible, 60.0, 60.0, 10.0, 1000.0, -1000.0);
        assert_eq!(x, 30.0);
        assert_eq!(y, 10.0);
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

// ============================================================================
// Chat Bubble Helpers (GlyphPulse / Quantum style)
// ============================================================================

const NSTRACKING_MOUSE_ENTERED_AND_EXITED: u64 = 1 << 0;
const NSTRACKING_ACTIVE_ALWAYS: u64 = 1 << 7;
const NSTRACKING_IN_VISIBLE_RECT: u64 = 1 << 9;

/// Role for chat bubble styling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleRole {
    User,
    Assistant,
    System,
}

fn is_markdown_table_separator_line(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|').trim();
    if trimmed.is_empty() || !trimmed.contains('-') {
        return false;
    }
    trimmed.split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
    })
}

fn looks_like_markdown_table(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.windows(2).any(|pair| {
        let header = pair[0];
        let sep = pair[1];
        header.contains('|') && is_markdown_table_separator_line(sep)
    })
}

fn markdown_options_with_base_font(text: &str, font: Id) -> Option<Id> {
    unsafe {
        let options_cls = Class::get("NSAttributedStringMarkdownParsingOptions")?;
        let options: Id = msg_send![options_cls, alloc];
        let options: Id = msg_send![options, init];
        if options.is_null() {
            return None;
        }
        let responds_base: bool = msg_send![options, respondsToSelector: sel!(setBaseFont:)];
        if responds_base && !font.is_null() {
            let _: () = msg_send![options, setBaseFont: font];
        }
        // Use full markdown mode when table syntax is detected; otherwise keep the
        // inline-preserving mode to avoid whitespace regressions in regular bubbles.
        let responds_syntax: bool =
            msg_send![options, respondsToSelector: sel!(setInterpretedSyntax:)];
        if responds_syntax {
            // 0 = .full, 1 = .inlineOnly, 2 = .inlineOnlyPreservingWhitespace
            let syntax: isize = if looks_like_markdown_table(text) {
                0
            } else {
                2
            };
            let _: () = msg_send![options, setInterpretedSyntax: syntax];
        }
        Some(options)
    }
}

/// NSRange for Objective-C attributed string APIs.
#[repr(C)]
#[derive(Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

// NSFontTraitMask bits (subset).
const NS_ITALIC_FONT_MASK: u64 = 1 << 0;
const NS_BOLD_FONT_MASK: u64 = 1 << 1;

/// Normalize per-range font attributes to stay within the provided base font family.
///
/// AppKit's Markdown parser may introduce different font families for inline `code` spans or
/// emphasis runs. We want consistent typography inside bubbles, while preserving bold/italic
/// traits and point sizes.
///
/// Returns an attributed string instance (possibly mutable) that is safe to set on
/// `NSTextField.setAttributedStringValue:`.
unsafe fn normalize_attributed_string_fonts(attr: Id, base_font: Id) -> Id {
    if attr.is_null() || base_font.is_null() {
        return attr;
    }

    let mutable: Id = msg_send![attr, mutableCopy];
    if mutable.is_null() {
        return attr;
    }
    // Release original — we now own the mutable copy exclusively.
    let _: () = msg_send![attr, release];

    let len: usize = msg_send![mutable, length];
    if len == 0 {
        return mutable;
    }

    let Some(ns_font_manager) = Class::get("NSFontManager") else {
        return mutable;
    };
    let fm: Id = msg_send![ns_font_manager, sharedFontManager];
    if fm.is_null() {
        return mutable;
    }

    let font_key = ns_string("NSFont");
    let base_family: Id = msg_send![base_font, familyName];
    let mut idx: usize = 0;
    while idx < len {
        let mut effective = NSRange {
            location: 0,
            length: 0,
        };
        let cur_font: Id = msg_send![
            mutable,
            attribute: font_key
            atIndex: idx
            effectiveRange: &mut effective
        ];
        if effective.length == 0 {
            idx += 1;
            continue;
        }

        if cur_font.is_null() {
            // Some markdown runs may not carry an explicit NSFont attribute.
            // Ensure those ranges still inherit the bubble's monospace base font
            // instead of NSTextField defaulting them to system UI font.
            let _: () =
                msg_send![mutable, addAttribute: font_key value: base_font range: effective];
            idx = effective.location + effective.length;
            continue;
        }

        if !cur_font.is_null() {
            let traits: u64 = msg_send![fm, traitsOfFont: cur_font];
            let desired_traits = traits & (NS_ITALIC_FONT_MASK | NS_BOLD_FONT_MASK);

            let cur_size: f64 = msg_send![cur_font, pointSize];
            let base_size: f64 = msg_send![base_font, pointSize];

            let mut new_font: Id = base_font;
            if (cur_size - base_size).abs() > 0.05 {
                let sized: Id = msg_send![fm, convertFont: base_font toSize: cur_size];
                if !sized.is_null() {
                    new_font = sized;
                }
            }
            if desired_traits != 0 {
                let converted: Id =
                    msg_send![fm, convertFont: new_font toHaveTrait: desired_traits];
                if !converted.is_null() {
                    // NSFontManager may fallback to a proportional system family when the
                    // requested trait isn't available in the base family. Keep monospace family
                    // stable for chat bubbles, even if that means dropping a trait.
                    let converted_family: Id = msg_send![converted, familyName];
                    let same_family: bool = if base_family.is_null() || converted_family.is_null() {
                        false
                    } else {
                        msg_send![base_family, isEqualToString: converted_family]
                    };
                    if same_family {
                        new_font = converted;
                    }
                }
            }

            let _: () = msg_send![mutable, addAttribute: font_key value: new_font range: effective];
        }

        idx = effective.location + effective.length;
    }

    mutable
}

unsafe fn markdown_attributed_string(text: &str, font: Id) -> Option<Id> {
    let ns_attr = Class::get("NSAttributedString")?;
    let text_ns = ns_string(text);
    let options =
        markdown_options_with_base_font(text, font).unwrap_or(std::ptr::null_mut::<Object>());

    // initWithMarkdown: expects NSData, not NSString
    let utf8_encoding: usize = 4; // NSUTF8StringEncoding
    let text_data: Id = msg_send![text_ns, dataUsingEncoding: utf8_encoding];
    if text_data.is_null() {
        return None;
    }

    let supports_with_error: bool = msg_send![ns_attr, instancesRespondToSelector: sel!(initWithMarkdown:options:baseURL:error:)];
    if supports_with_error {
        let obj: Id = msg_send![ns_attr, alloc];
        let obj: Id = msg_send![
            obj,
            initWithMarkdown: text_data
            options: options
            baseURL: std::ptr::null::<Object>()
            error: std::ptr::null_mut::<*mut Object>()
        ];
        if !obj.is_null() {
            return Some(unsafe { normalize_attributed_string_fonts(obj, font) });
        }
    }

    let supports_simple: bool =
        msg_send![ns_attr, instancesRespondToSelector: sel!(initWithMarkdown:options:baseURL:)];
    if supports_simple {
        let obj: Id = msg_send![ns_attr, alloc];
        let obj: Id = msg_send![
            obj,
            initWithMarkdown: text_data
            options: options
            baseURL: std::ptr::null::<Object>()
        ];
        if !obj.is_null() {
            return Some(unsafe { normalize_attributed_string_fonts(obj, font) });
        }
    }

    None
}

unsafe fn apply_markdown_to_text_field(text_label: Id, text: &str, font: Id) -> bool {
    let responds_attr: bool =
        msg_send![text_label, respondsToSelector: sel!(setAttributedStringValue:)];
    if !responds_attr {
        return false;
    }
    let font = if font.is_null() {
        let ns_font = Class::get("NSFont").unwrap();
        msg_send![ns_font, systemFontOfSize: 13.0f64]
    } else {
        font
    };
    // Keep NSTextField fallback font aligned with markdown base font.
    let _: () = msg_send![text_label, setFont: font];
    if let Some(attr) = unsafe { markdown_attributed_string(text, font) } {
        let _: () = msg_send![text_label, setAttributedStringValue: attr];
        // Re-assert base font after attributed update so any future fallback ranges
        // (e.g. during incremental updates) remain monospace.
        let _: () = msg_send![text_label, setFont: font];
        // Balance the +1 from mutableCopy inside normalize_attributed_string_fonts.
        // setAttributedStringValue: retains its own copy.
        let _: () = msg_send![attr, release];
        return true;
    }
    false
}

/// Configuration for creating a chat bubble
pub struct BubbleConfig {
    pub text: String,
    pub role: BubbleRole,
    pub max_width: f64,
    pub font_size: f64,
    pub is_streaming: bool,
    pub is_error: bool,
    pub metadata: Option<String>,
    /// Optional message index for Copy button (None = no button)
    pub message_index: Option<usize>,
    /// Optional action target for Copy button
    pub copy_action_target: Option<Id>,
}

/// Create a chat bubble view (NSView container with styled text)
///
/// Returns (container_view, text_label) tuple for later updates
pub fn create_bubble_view(config: BubbleConfig) -> (Id, Id) {
    unsafe {
        let ns_view = bubble_container_view_class();
        let ns_text_field = bubble_text_field_class();
        let ns_font = Class::get("NSFont").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();

        let font_size = config.font_size;
        let padding_x = 12.0;
        let padding_top = 10.0;
        let copy_button_height = if config.message_index.is_some() {
            16.0
        } else {
            0.0
        };
        // Reserve space for the Copy button so it never overlaps text.
        let padding_bottom = if copy_button_height > 0.0 {
            copy_button_height + 8.0
        } else {
            10.0
        };
        let line_height = font_size * 1.4;
        let meta_height = if config.metadata.is_some() {
            (ui_tokens::SMALL_FONT_SIZE + 4.0).max(12.0)
        } else {
            0.0
        };
        let meta_spacing = if config.metadata.is_some() { 4.0 } else { 0.0 };

        // Font (prefer JetBrains Mono if installed)
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font, fontWithName: jb_name size: font_size];
        let font: Id = if jb_font.is_null() {
            msg_send![ns_font, monospacedSystemFontOfSize: font_size weight: 0.0f64]
        } else {
            jb_font
        };

        // Set text (with streaming indicator if needed)
        let display_text = if config.is_streaming && config.text.is_empty() {
            "• • •".to_string() // Pulsing dots placeholder
        } else if config.is_streaming {
            format!("{} …", config.text)
        } else {
            config.text.clone()
        };

        // Measure text height/width using NSString boundingRectWithSize (handles newlines/wrapping).
        //
        // NOTE: `NSFontAttributeName` (key) has the string value "NSFont". AppKit expects that
        // key, not the literal "NSFontAttributeName" string.
        let text_str = ns_string(&display_text);
        let font_key = ns_string("NSFont");
        let attrs: Id = msg_send![ns_dict, dictionaryWithObject: font forKey: font_key];
        let opts: u64 = 1 | 2; // NSStringDrawingUsesLineFragmentOrigin | NSStringDrawingUsesFontLeading

        // Keep a small side margin inside the container so full-width bubbles don't overflow.
        let bubble_max_width = (config.max_width - 16.0).max(80.0);
        let text_max_width = (bubble_max_width - padding_x * 2.0).max(40.0);
        let rect_max: CGRect = msg_send![
            text_str,
            boundingRectWithSize: CGSize::new(text_max_width, 10_000.0)
            options: opts
            attributes: attrs
        ];

        // Bubble width: content-aware but capped.
        // If it wraps (or is long), keep the bubble full width for readability.
        //
        // We treat streaming messages as "wrap-prone" earlier to avoid the initial narrow bubble
        // that later expands mid-stream.
        let long_threshold = if config.is_streaming { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;
        let wraps_at_max = rect_max.size.height > line_height * 1.6
            || display_text.contains('\n')
            || is_long
            // When streaming starts with the "• • •" placeholder, force full-width bubbles
            // to avoid the initial tiny/narrow bubble that later expands mid-stream.
            || (config.is_streaming && config.text.is_empty());
        let bubble_width = if wraps_at_max {
            bubble_max_width
        } else {
            let content_width = rect_max.size.width.min(text_max_width).max(1.0);
            (content_width + padding_x * 2.0).min(bubble_max_width)
        };

        // Label width for wrapping and later reflow.
        let text_layout_width = (bubble_width - padding_x * 2.0).max(40.0);

        // Build the label first and ask AppKit (cell) for the exact wrapped height.
        // This avoids "second line appears only after click" issues where our NSString
        // measurement disagrees with NSTextField's rendering.
        let text_label: Id = msg_send![ns_text_field, alloc];
        let text_label: Id = msg_send![
            text_label,
            initWithFrame: CGRect::new(
                &CGPoint::new(padding_x, padding_top),
                &CGSize::new(text_layout_width.max(1.0), line_height),
            )
        ];

        let _: () = msg_send![text_label, setBezeled: false];
        let _: () = msg_send![text_label, setEditable: false];
        let _: () = msg_send![text_label, setSelectable: true];
        let _: () = msg_send![text_label, setDrawsBackground: false];
        let _: () = msg_send![text_label, setUsesSingleLineMode: false];
        let _: () = msg_send![text_label, setRefusesFirstResponder: false];
        let responds_attr: bool =
            msg_send![text_label, respondsToSelector: sel!(setAllowsEditingTextAttributes:)];
        if responds_attr {
            let _: () = msg_send![text_label, setAllowsEditingTextAttributes: false];
        }

        // Enable wrapping for multi-line messages.
        let cell: Id = msg_send![text_label, cell];
        if !cell.is_null() {
            let _: () = msg_send![cell, setWraps: true];
            let _: () = msg_send![cell, setLineBreakMode: 0_isize]; // NSLineBreakByWordWrapping
            let _: () = msg_send![cell, setScrollable: false];
        }

        // Text color (role-aware)
        let text_color: Id = if config.is_error {
            ui_colors::bubble_error_text()
        } else {
            match config.role {
                BubbleRole::User => ui_colors::bubble_text(),
                BubbleRole::Assistant => {
                    if config.is_streaming {
                        ui_colors::bubble_streaming_text()
                    } else {
                        ui_colors::bubble_text()
                    }
                }
                BubbleRole::System => ui_colors::bubble_text(),
            }
        };
        let _: () = msg_send![text_label, setTextColor: text_color];

        let _: () = msg_send![text_label, setFont: font];
        let allow_markdown = matches!(config.role, BubbleRole::Assistant | BubbleRole::System);
        if !(allow_markdown && apply_markdown_to_text_field(text_label, &display_text, font)) {
            let _: () = msg_send![text_label, setStringValue: text_str];
        }
        let _: () = msg_send![text_label, setLineBreakMode: 0_isize]; // NSLineBreakByWordWrapping

        // Ask the cell for the wrapped size within the fixed width.
        let measure_bounds = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(text_layout_width.max(1.0), 10_000.0),
        );
        let cell: Id = msg_send![text_label, cell];
        let measured: CGSize = if cell.is_null() {
            // Fallback to NSString measurement (best effort).
            let text_rect: CGRect = msg_send![
                text_str,
                boundingRectWithSize: CGSize::new(text_layout_width, 10_000.0)
                options: opts
                attributes: attrs
            ];
            text_rect.size
        } else {
            msg_send![cell, cellSizeForBounds: measure_bounds]
        };
        let text_height = measured.height.ceil().max(line_height);
        let bubble_height = text_height + padding_top + padding_bottom;
        let container_height = bubble_height + meta_height + meta_spacing;

        // Container view (for alignment)
        let container: Id = msg_send![ns_view, alloc];
        let container_frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(config.max_width, container_height),
        );
        let container: Id = msg_send![container, initWithFrame: container_frame];

        // Bubble background view
        let bubble: Id = msg_send![ns_view, alloc];
        let bubble_x = match config.role {
            BubbleRole::User => (config.max_width - bubble_width - 8.0).max(8.0), // Right-aligned
            BubbleRole::Assistant | BubbleRole::System => 8.0,                    // Left-aligned
        };
        let bubble_y = meta_height + meta_spacing;
        let bubble_frame = CGRect::new(
            &CGPoint::new(bubble_x, bubble_y),
            &CGSize::new(bubble_width, bubble_height),
        );
        let bubble: Id = msg_send![bubble, initWithFrame: bubble_frame];

        // Set bubble background color based on role
        let bg_color: Id = if config.is_error {
            ui_colors::bubble_error_bg()
        } else {
            match config.role {
                BubbleRole::User => ui_colors::bubble_user_bg(),
                BubbleRole::Assistant => ui_colors::bubble_assistant_bg(),
                BubbleRole::System => ui_colors::bubble_system_bg(),
            }
        };

        // Set background via layer (for rounded corners)
        let _: () = msg_send![bubble, setWantsLayer: true];
        let layer: Id = msg_send![bubble, layer];
        if !layer.is_null() {
            // CGColor from NSColor
            let cg_color: Id = msg_send![bg_color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            apply_tafla_surface(layer, false);
            let _: () = msg_send![layer, setMasksToBounds: false];
            // Border styling
            let (border_color, bw) = if config.is_error {
                (
                    ui_colors::bubble_error_text(),
                    ui_tokens::SURFACE_BORDER_WIDTH,
                )
            } else {
                match config.role {
                    BubbleRole::User => (
                        ui_colors::bubble_user_border(),
                        ui_tokens::SURFACE_BORDER_WIDTH,
                    ),
                    BubbleRole::Assistant | BubbleRole::System => {
                        (ui_colors::bubble_border(), ui_tokens::SURFACE_BORDER_WIDTH)
                    }
                }
            };
            if bw > 0.0 {
                let cg_border: Id = msg_send![border_color, CGColor];
                let _: () = msg_send![layer, setBorderColor: cg_border];
                let _: () = msg_send![layer, setBorderWidth: bw];
            }
        }

        // Update label frame to the final measured height.
        let text_frame = CGRect::new(
            &CGPoint::new(padding_x, padding_top),
            &CGSize::new(text_layout_width.max(1.0), text_height),
        );
        let _: () = msg_send![text_label, setFrame: text_frame];
        add_subview(bubble, text_label);

        // Metadata (role/time/mode) above the bubble.
        if let Some(meta) = config.metadata.as_ref() {
            let meta_label: Id = msg_send![ns_text_field, alloc];
            let meta_frame = CGRect::new(
                &CGPoint::new(bubble_x, 0.0),
                &CGSize::new(bubble_width.max(1.0), meta_height.max(1.0)),
            );
            let meta_label: Id = msg_send![meta_label, initWithFrame: meta_frame];
            let _: () = msg_send![meta_label, setBezeled: false];
            let _: () = msg_send![meta_label, setEditable: false];
            let _: () = msg_send![meta_label, setSelectable: false];
            let _: () = msg_send![meta_label, setDrawsBackground: false];

            let meta_font: Id = msg_send![ns_font, systemFontOfSize: ui_tokens::SMALL_FONT_SIZE];
            let _: () = msg_send![meta_label, setFont: meta_font];
            let meta_color: Id = ui_colors::bubble_meta_text();
            let _: () = msg_send![meta_label, setTextColor: meta_color];

            let alignment: isize = if config.role == BubbleRole::User {
                2
            } else {
                0
            };
            let _: () = msg_send![meta_label, setAlignment: alignment];
            let _: () = msg_send![meta_label, setStringValue: ns_string(meta)];
            let _: () = msg_send![meta_label, setIdentifier: ns_string("codescribe_bubble_meta")];

            let _: () = msg_send![container, addSubview: meta_label];
        }

        // Assemble hierarchy
        // (text_label already added to bubble above — directly or via scroll wrapper)
        // Add Copy button if message_index is provided
        if let (Some(msg_index), Some(target)) = (config.message_index, config.copy_action_target) {
            let ns_button = Class::get("NSButton").unwrap();

            let button_width = 40.0;
            let button_height = copy_button_height;
            let button_x = bubble_width - button_width - padding_x;
            // Flipped coords: anchor near the bottom edge.
            let button_y = (bubble_height - button_height - 4.0).max(4.0);

            let button_frame = CGRect::new(
                &CGPoint::new(button_x, button_y),
                &CGSize::new(button_width, button_height),
            );

            let copy_button: Id = msg_send![ns_button, alloc];
            let copy_button: Id = msg_send![copy_button, initWithFrame: button_frame];

            // Style: small borderless button
            let _: () = msg_send![copy_button, setBezelStyle: 0_isize]; // NSBezelStyleRounded
            let _: () = msg_send![copy_button, setBordered: false];

            // Title "Copy" in small font
            let title = ns_string("Copy");
            let _: () = msg_send![copy_button, setTitle: title];

            let small_font: Id = if jb_font.is_null() {
                msg_send![ns_font, monospacedSystemFontOfSize: 10.0f64 weight: 0.0f64]
            } else {
                msg_send![ns_font, fontWithName: jb_name size: 10.0f64]
            };
            let _: () = msg_send![copy_button, setFont: small_font];

            // Match bubble text tint
            let button_color: Id = ui_colors::bubble_text();
            let _: () = msg_send![copy_button, setContentTintColor: button_color];

            // Store message index in tag for retrieval on click
            let _: () = msg_send![copy_button, setTag: msg_index as isize];
            let _: () = msg_send![
                copy_button,
                setIdentifier: ns_string("codescribe_copy_button")
            ];

            // Set action
            let _: () = msg_send![copy_button, setTarget: target];
            let _: () = msg_send![copy_button, setAction: sel!(onCopyMessage:)];

            let _: () = msg_send![copy_button, setHidden: true];
            let _: () = msg_send![bubble, addSubview: copy_button];
        }

        let _: () = msg_send![container, addSubview: bubble];

        if config.message_index.is_some() {
            let ns_tracking_area = Class::get("NSTrackingArea").unwrap();
            let tracking_opts = NSTRACKING_MOUSE_ENTERED_AND_EXITED
                | NSTRACKING_ACTIVE_ALWAYS
                | NSTRACKING_IN_VISIBLE_RECT;
            let tracking_area: Id = msg_send![ns_tracking_area, alloc];
            let tracking_area: Id = msg_send![
                tracking_area,
                initWithRect: CGRect::new(
                    &CGPoint::new(0.0, 0.0),
                    &CGSize::new(bubble_width.max(1.0), bubble_height.max(1.0)),
                )
                options: tracking_opts
                owner: bubble
                userInfo: std::ptr::null::<Object>()
            ];
            let _: () = msg_send![bubble, addTrackingArea: tracking_area];
        }

        (container, text_label)
    }
}

/// Update bubble text (for streaming updates)
/// # Safety
/// `text_label` must be a valid `NSTextField` instance.
pub unsafe fn update_bubble_text(
    text_label: Id,
    text: &str,
    role: BubbleRole,
    is_streaming: bool,
    is_error: bool,
) {
    unsafe {
        let display_text = if is_streaming && text.is_empty() {
            "• • •".to_string()
        } else if is_streaming {
            format!("{} …", text)
        } else {
            text.to_string()
        };

        let allow_markdown = matches!(role, BubbleRole::Assistant | BubbleRole::System);
        // Always create a fresh monospace font instead of reading from the label.
        // After markdown parsing, text_label.font may return a system font from
        // the first attributed range, causing cascading degradation on subsequent updates.
        let label_font: Id = msg_send![text_label, font];
        let font_size: f64 = if label_font.is_null() {
            13.0
        } else {
            msg_send![label_font, pointSize]
        };
        let ns_font_cls = Class::get("NSFont").unwrap();
        let jb_name = ns_string("JetBrainsMono-Regular");
        let jb_font: Id = msg_send![ns_font_cls, fontWithName: jb_name size: font_size];
        let font: Id = if jb_font.is_null() {
            msg_send![ns_font_cls, monospacedSystemFontOfSize: font_size weight: 0.0f64]
        } else {
            jb_font
        };
        let _: () = msg_send![text_label, setFont: font];
        if !(allow_markdown && apply_markdown_to_text_field(text_label, &display_text, font)) {
            let text_str = ns_string(&display_text);
            let _: () = msg_send![text_label, setStringValue: text_str];
        }

        let text_color: Id = if is_error {
            ui_colors::bubble_error_text()
        } else {
            match role {
                BubbleRole::User => ui_colors::bubble_text(),
                BubbleRole::Assistant => {
                    if is_streaming {
                        ui_colors::bubble_streaming_text()
                    } else {
                        ui_colors::bubble_text()
                    }
                }
                BubbleRole::System => ui_colors::bubble_text(),
            }
        };
        let _: () = msg_send![text_label, setTextColor: text_color];
    }
}

/// Update a stack view item (bubble container) height constraint if present.
///
/// `stack_view_add` installs a fixed-height constraint on each arranged subview.
/// During streaming, the bubble text grows and we need to update that constraint
/// so the view doesn't clip.
///
/// # Safety
/// `view` must be a valid `NSView` instance.
pub unsafe fn update_stack_item_height(view: Id, new_height: f64) {
    unsafe {
        let constraints: Id = msg_send![view, constraints];
        if constraints.is_null() {
            return;
        }
        let count: usize = msg_send![constraints, count];
        for i in 0..count {
            let c: Id = msg_send![constraints, objectAtIndex: i];
            if c.is_null() {
                continue;
            }

            // Prefer our tagged constraint.
            let ident: Id = msg_send![c, identifier];
            if !ident.is_null() {
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if !c_str.is_null() {
                    let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                    if s == "codescribe_height" {
                        let _: () = msg_send![c, setConstant: new_height];
                        return;
                    }
                }
            }

            // Fallback: find a height constraint on this view.
            let first: Id = msg_send![c, firstItem];
            if first != view {
                continue;
            }
            let second: Id = msg_send![c, secondItem];
            if !second.is_null() {
                continue;
            }
            let first_attr: isize = msg_send![c, firstAttribute];
            // NSLayoutAttributeHeight == 8
            if first_attr == 8 {
                let _: () = msg_send![c, setConstant: new_height];
                return;
            }
        }
    }
}

/// Resize an existing bubble container + its internal views for the given text.
///
/// Used for streaming updates to prevent clipping without rebuilding the whole view tree.
///
/// # Safety
/// `container` must be the container returned by `create_bubble_view`.
/// `text_label` must be the label returned by `create_bubble_view`.
pub unsafe fn resize_bubble_container_for_text(container: Id, text_label: Id, display_text: &str) {
    unsafe {
        let ns_font = Class::get("NSFont").unwrap();

        let font: Id = msg_send![text_label, font];
        let font = if font.is_null() {
            msg_send![ns_font, systemFontOfSize: 13.0f64]
        } else {
            font
        };

        let container_frame: CGRect = msg_send![container, frame];
        let max_width = container_frame.size.width.max(80.0);
        let bubble_max_width = (max_width - 16.0).max(80.0);

        // If the message is getting long, switch to full-width to avoid one-word-per-line bubbles.
        //
        // During streaming we append " …" so we can detect it and widen earlier to prevent
        // the initial narrow bubble phase.
        let streaming_like = display_text.ends_with('…');
        let long_threshold = if streaming_like { 30 } else { 80 };
        let is_long = display_text.chars().count() > long_threshold;
        let force_full_width = display_text.contains('\n') || is_long;

        let label_frame: CGRect = msg_send![text_label, frame];
        let width = if force_full_width {
            let padding_x = 12.0;
            (bubble_max_width - padding_x * 2.0).max(40.0)
        } else {
            label_frame.size.width.max(1.0)
        };

        // Approximate line-height floor to avoid tiny/bad measurements.
        let point_size: f64 = msg_send![font, pointSize];
        let line_height = (point_size * 1.35).max(14.0);

        // Match `create_bubble_view` layout constants.
        let padding_top = 10.0;
        let copy_button_height = 16.0;
        let padding_bottom = copy_button_height + 8.0;

        // Ask the label's cell for the wrapped height in the current width.
        let measure_bounds = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width.max(1.0), 10_000.0),
        );
        let cell: Id = msg_send![text_label, cell];
        let measured: CGSize = if cell.is_null() {
            // Fallback to a conservative single line height.
            CGSize::new(width.max(1.0), line_height)
        } else {
            msg_send![cell, cellSizeForBounds: measure_bounds]
        };
        let text_height = measured.height.ceil().max(line_height);
        let bubble_height = text_height + padding_top + padding_bottom;
        let mut meta_height = 0.0;
        let mut meta_spacing = 0.0;
        let mut meta_label: Option<Id> = None;

        let subviews: Id = msg_send![container, subviews];
        if !subviews.is_null() {
            let sub_count: usize = msg_send![subviews, count];
            for i in 0..sub_count {
                let v: Id = msg_send![subviews, objectAtIndex: i];
                if v.is_null() {
                    continue;
                }
                let ident: Id = msg_send![v, identifier];
                if ident.is_null() {
                    continue;
                }
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if c_str.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(c_str).to_string_lossy();
                if s == "codescribe_bubble_meta" {
                    let frame: CGRect = msg_send![v, frame];
                    meta_height = frame.size.height.max(ui_tokens::SMALL_FONT_SIZE);
                    meta_spacing = 4.0;
                    meta_label = Some(v);
                    break;
                }
            }
        }

        // Resize bubble background view (label's superview).
        let bubble: Id = msg_send![text_label, superview];
        if !bubble.is_null() {
            let bubble_frame: CGRect = msg_send![bubble, frame];
            let mut bubble_width = bubble_frame.size.width;
            let mut bubble_x = bubble_frame.origin.x;

            if force_full_width {
                bubble_width = bubble_max_width;
                // Preserve alignment based on prior x (user bubbles are right-aligned).
                let was_right_aligned = bubble_x > 20.0;
                bubble_x = if was_right_aligned {
                    (max_width - bubble_width - 8.0).max(8.0)
                } else {
                    8.0
                };
            }

            // Resize label to match bubble width (keep in sync with create_bubble_view).
            let padding_x = 12.0;
            let new_label_w = (bubble_width - padding_x * 2.0).max(1.0);
            let new_label_frame = CGRect::new(
                &CGPoint::new(padding_x, padding_top),
                &CGSize::new(new_label_w, text_height),
            );
            let _: () = msg_send![text_label, setFrame: new_label_frame];

            if let Some(meta_ptr) = meta_label {
                let meta_frame = CGRect::new(
                    &CGPoint::new(bubble_x, 0.0),
                    &CGSize::new(bubble_width.max(1.0), meta_height.max(1.0)),
                );
                let _: () = msg_send![meta_ptr, setFrame: meta_frame];
            }

            // Reposition the Copy button to stay anchored near the bottom edge (flipped coords).
            let ns_button = Class::get("NSButton").unwrap();
            let subviews: Id = msg_send![bubble, subviews];
            if !subviews.is_null() {
                let sub_count: usize = msg_send![subviews, count];
                for i in 0..sub_count {
                    let v: Id = msg_send![subviews, objectAtIndex: i];
                    if v.is_null() {
                        continue;
                    }
                    let is_button: bool = msg_send![v, isKindOfClass: ns_button];
                    if !is_button {
                        continue;
                    }
                    let btn_frame: CGRect = msg_send![v, frame];
                    let btn_h = btn_frame.size.height;
                    let new_y = (bubble_height - btn_h - 4.0).max(4.0);
                    let new_frame = CGRect::new(
                        &CGPoint::new(btn_frame.origin.x, new_y),
                        &CGSize::new(btn_frame.size.width, btn_frame.size.height),
                    );
                    let _: () = msg_send![v, setFrame: new_frame];
                }
            }

            let bubble_y = if meta_height > 0.0 {
                meta_height + meta_spacing
            } else {
                bubble_frame.origin.y
            };
            let new_bubble_frame = CGRect::new(
                &CGPoint::new(bubble_x, bubble_y),
                &CGSize::new(bubble_width, bubble_height),
            );
            let _: () = msg_send![bubble, setFrame: new_bubble_frame];
            let _: () = msg_send![bubble, setNeedsDisplay: true];
            let _: () = msg_send![text_label, setNeedsDisplay: true];
        }

        // Resize container (stack arranged subview).
        let container_height = bubble_height + meta_height + meta_spacing;
        let _: () = msg_send![
            container,
            setFrameSize: CGSize::new(container_frame.size.width, container_height)
        ];
        update_stack_item_height(container, container_height);

        let _: () = msg_send![container, setNeedsLayout: true];
        let _: () = msg_send![container, layoutSubtreeIfNeeded];
        let _: () = msg_send![container, setNeedsDisplay: true];

        // NSStackView (superview) does the actual arrangement; ensure it reflows immediately
        // so updated height constraints take effect without requiring a click/focus change.
        let stack: Id = msg_send![container, superview];
        if !stack.is_null() {
            let _: () = msg_send![stack, setNeedsLayout: true];
            let _: () = msg_send![stack, layoutSubtreeIfNeeded];
        }
    }
}

// ============================================================================
// File Operations Helpers
// ============================================================================

/// Pick one or more files via native macOS open panel.
///
/// Returns absolute paths. Intended for "attach as context" flows (Agent chat).
pub fn pick_files_open_panel(title: &str) -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    unsafe {
        let Some(ns_open_panel) = Class::get("NSOpenPanel") else {
            return Vec::new();
        };
        let panel: Id = msg_send![ns_open_panel, openPanel];
        if panel.is_null() {
            return Vec::new();
        }

        let _: () = msg_send![panel, setTitle: ns_string(title)];
        let _: () = msg_send![panel, setCanChooseFiles: true];
        let _: () = msg_send![panel, setCanChooseDirectories: false];
        let _: () = msg_send![panel, setAllowsMultipleSelection: true];

        // Prefer predictable prompt text (keeps UX clear).
        let _: () = msg_send![panel, setPrompt: ns_string("Attach")];

        // runModal returns NSModalResponse (NSModalResponseOK == 1).
        let resp: isize = msg_send![panel, runModal];
        if resp != 1 {
            return Vec::new();
        }

        let urls: Id = msg_send![panel, URLs];
        if urls.is_null() {
            return Vec::new();
        }

        let count: usize = msg_send![urls, count];
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let url: Id = msg_send![urls, objectAtIndex: i];
            if url.is_null() {
                continue;
            }
            let ns_path: Id = msg_send![url, path];
            if ns_path.is_null() {
                continue;
            }
            let c_str: *const i8 = msg_send![ns_path, UTF8String];
            if c_str.is_null() {
                continue;
            }
            let s = std::ffi::CStr::from_ptr(c_str)
                .to_string_lossy()
                .to_string();
            if s.is_empty() {
                continue;
            }
            out.push(std::path::PathBuf::from(s));
        }
        out
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        Vec::new()
    }
}

/// Open a file in the default editor (TextEdit, etc.)
pub fn open_file_in_editor(path: &std::path::Path) -> bool {
    // Most reliable approach in the app-bundle environment: call `/usr/bin/open`.
    // NSWorkspace sometimes reports success but doesn't surface the editor window. `open -e`
    // (TextEdit) is predictable and works without PATH.
    #[cfg(target_os = "macos")]
    {
        use std::time::Duration;
        use tracing::{info, warn};

        let path = path.to_path_buf();
        if !path.exists() {
            warn!(
                "open_file_in_editor: path does not exist: {}",
                path.display()
            );
            return false;
        }

        let open_via_nsworkspace_textedit = || -> bool {
            unsafe {
                let ns_workspace = match Class::get("NSWorkspace") {
                    Some(c) => c,
                    None => return false,
                };
                let workspace: Id = msg_send![ns_workspace, sharedWorkspace];
                if workspace.is_null() {
                    return false;
                }

                let path_str = path.to_string_lossy();
                let ns_path = ns_string(&path_str);
                let app = ns_string("TextEdit");

                // Prefer explicit app open (avoids "Open…" panel / wrong default handler).
                let ok: bool = msg_send![workspace, openFile: ns_path withApplication: app];
                info!("NSWorkspace openFile:withApplication(TextEdit) ok={}", ok);
                ok
            }
        };

        let run_open = |args: &[&str]| -> bool {
            let out = std::process::Command::new("/usr/bin/open")
                .args(args)
                .arg(&path)
                .output();
            match out {
                Ok(out) => {
                    let code = out.status.code().unwrap_or(-1);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !stderr.trim().is_empty() {
                        info!(
                            "open {:?} exit={} stderr={}",
                            args,
                            code,
                            stderr.trim().replace('\n', "\\n")
                        );
                    } else {
                        info!("open {:?} exit={}", args, code);
                    }
                    out.status.success()
                }
                Err(e) => {
                    warn!("open {:?} failed to spawn: {}", args, e);
                    false
                }
            }
        };

        let activate_textedit_best_effort = || {
            // Try to bring TextEdit to the foreground without requiring Automation permissions
            // (osascript can trigger a prompt / fail silently).
            unsafe {
                let ns_running_app = match Class::get("NSRunningApplication") {
                    Some(c) => c,
                    None => return,
                };
                let bundle_id = ns_string("com.apple.TextEdit");
                let apps: Id =
                    msg_send![ns_running_app, runningApplicationsWithBundleIdentifier: bundle_id];
                if apps.is_null() {
                    return;
                }

                let count: usize = msg_send![apps, count];
                if count == 0 {
                    return;
                }

                // NSApplicationActivateAllWindows (1) | NSApplicationActivateIgnoringOtherApps (2)
                let opts: u64 = 3;
                for i in 0..count {
                    let app: Id = msg_send![apps, objectAtIndex: i];
                    if app.is_null() {
                        continue;
                    }
                    let ok: bool = msg_send![app, activateWithOptions: opts];
                    info!("TextEdit activateWithOptions result={}", ok);
                }
            }
        };

        // Force TextEdit and try to surface it; otherwise it can open "somewhere" (another Space)
        // and look like a no-op from the user's POV.
        // Prefer `open -a TextEdit <file>` (explicit app + file). Fallback to `-e` if needed.
        if open_via_nsworkspace_textedit() || run_open(&["-a", "TextEdit"]) || run_open(&["-e"]) {
            // Give launch a moment so NSRunningApplication can see the process.
            std::thread::sleep(Duration::from_millis(75));
            activate_textedit_best_effort();
            return true;
        }
        if run_open(&["-t"]) || run_open(&[]) {
            return true;
        }
    }

    unsafe {
        let ns_workspace = Class::get("NSWorkspace").unwrap();
        let workspace: Id = msg_send![ns_workspace, sharedWorkspace];

        let path_str = path.to_string_lossy();
        let ns_path = ns_string(&path_str);

        let ok: bool = msg_send![workspace, openFile: ns_path];
        if ok {
            return true;
        }

        // Fallback: open via file:// URL (some apps prefer this path).
        let ns_url = Class::get("NSURL").unwrap();
        let url: Id = msg_send![ns_url, fileURLWithPath: ns_path];
        if url.is_null() {
            // last resort below (shell open)
        } else {
            let ok2: bool = msg_send![workspace, openURL: url];
            if ok2 {
                return true;
            }
        }
    }

    let _ = path;
    false
}

/// List draft files from a directory, sorted by modification time (newest first)
pub fn list_draft_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|ext| ext == "txt" || ext == "md")
        })
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();

    // Sort by modification time, newest first
    files.sort_by(|a, b| b.1.cmp(&a.1));

    files.into_iter().map(|(path, _)| path).collect()
}

// ============================================================================
// NSStackView Helpers
// ============================================================================

/// Create a vertical NSStackView for stacking views
pub fn create_vertical_stack_view(frame: CGRect) -> Id {
    unsafe {
        let ns_stack_view = Class::get("NSStackView").unwrap();

        let stack: Id = msg_send![ns_stack_view, alloc];
        let stack: Id = msg_send![stack, initWithFrame: frame];

        // Vertical orientation (1 = NSUserInterfaceLayoutOrientationVertical)
        let _: () = msg_send![stack, setOrientation: 1_isize];
        // Top alignment
        let _: () = msg_send![stack, setAlignment: 1_isize]; // NSLayoutAttributeLeft
        // Spacing between views
        let _: () = msg_send![stack, setSpacing: 8.0f64];

        stack
    }
}

/// Create a flipped vertical NSStackView (y-axis grows downward).
///
/// This is useful for chat-like UIs where we want "top-down" coordinates and stable bubble
/// sizing during streaming.
pub fn create_flipped_vertical_stack_view(frame: CGRect) -> Id {
    unsafe {
        let ns_stack_view = flipped_stack_view_class();

        let stack: Id = msg_send![ns_stack_view, alloc];
        let stack: Id = msg_send![stack, initWithFrame: frame];

        // Vertical orientation (1 = NSUserInterfaceLayoutOrientationVertical)
        let _: () = msg_send![stack, setOrientation: 1_isize];
        // Top alignment
        let _: () = msg_send![stack, setAlignment: 1_isize]; // NSLayoutAttributeLeft
        // Spacing between views
        let _: () = msg_send![stack, setSpacing: 8.0f64];

        stack
    }
}

fn flipped_stack_view_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSStackView").expect("NSStackView class missing");
        let mut decl = ClassDecl::new("CodeScribeFlippedStackView", superclass)
            .expect("CodeScribeFlippedStackView already defined");
        decl.add_method(
            sel!(isFlipped),
            is_flipped as extern "C" fn(&Object, Sel) -> bool,
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn is_flipped(_this: &Object, _cmd: Sel) -> bool {
    true
}

fn bubble_container_view_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSView").expect("NSView class missing");
        let mut decl = ClassDecl::new("CodeScribeBubbleContainerView", superclass)
            .expect("CodeScribeBubbleContainerView already defined");
        decl.add_method(
            sel!(isFlipped),
            is_flipped as extern "C" fn(&Object, Sel) -> bool,
        );
        decl.add_method(
            sel!(scrollWheel:),
            bubble_container_scroll_wheel as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseEntered:),
            bubble_container_mouse_entered as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(mouseExited:),
            bubble_container_mouse_exited as extern "C" fn(&Object, Sel, Id),
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn bubble_container_scroll_wheel(this: &Object, _cmd: Sel, event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        if view.is_null() || event.is_null() {
            return;
        }

        // When the pointer is over a bubble background, AppKit may not route wheel events to the
        // surrounding scroll view. Forward explicitly so long messages stay scrollable.
        let scroll: Id = msg_send![view, enclosingScrollView];
        if !scroll.is_null() {
            let _: () = msg_send![scroll, scrollWheel: event];
            return;
        }

        let next: Id = msg_send![view, nextResponder];
        if !next.is_null() {
            let _: () = msg_send![next, scrollWheel: event];
        }
    }
}

extern "C" fn bubble_container_mouse_entered(this: &Object, _cmd: Sel, _event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        toggle_bubble_copy_buttons(view, true);
    }
}

extern "C" fn bubble_container_mouse_exited(this: &Object, _cmd: Sel, _event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        toggle_bubble_copy_buttons(view, false);
    }
}

unsafe fn toggle_bubble_copy_buttons(view: Id, visible: bool) {
    let ns_button = Class::get("NSButton").unwrap();
    let subviews: Id = msg_send![view, subviews];
    if subviews.is_null() {
        return;
    }
    let count: usize = msg_send![subviews, count];
    for i in 0..count {
        let v: Id = msg_send![subviews, objectAtIndex: i];
        if v.is_null() {
            continue;
        }
        let is_button: bool = msg_send![v, isKindOfClass: ns_button];
        if is_button {
            let ident: Id = msg_send![v, identifier];
            if !ident.is_null() {
                let c_str: *const i8 = msg_send![ident, UTF8String];
                if !c_str.is_null() {
                    let s = unsafe { std::ffi::CStr::from_ptr(c_str) }.to_string_lossy();
                    if s == "codescribe_copy_button" {
                        let _: () = msg_send![v, setHidden: !visible];
                    }
                }
            }
            continue;
        }
        unsafe { toggle_bubble_copy_buttons(v, visible) };
    }
}

fn bubble_text_field_class() -> &'static Class {
    static mut CLS: *const Class = std::ptr::null();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let superclass = Class::get("NSTextField").expect("NSTextField class missing");
        let mut decl = ClassDecl::new("CodeScribeBubbleTextField", superclass)
            .expect("CodeScribeBubbleTextField already defined");
        decl.add_method(
            sel!(scrollWheel:),
            bubble_text_scroll_wheel as extern "C" fn(&Object, Sel, Id),
        );
        let cls = decl.register();
        CLS = cls as *const Class;
    });
    unsafe { &*CLS }
}

extern "C" fn bubble_text_scroll_wheel(this: &Object, _cmd: Sel, event: Id) {
    unsafe {
        let view: Id = (this as *const Object) as Id;
        if view.is_null() || event.is_null() {
            return;
        }

        // Selectable text fields sometimes "eat" scroll wheel events without scrolling anything.
        // Forward the wheel to the enclosing scroll view so Agent/Drawer can always scroll.
        let scroll: Id = msg_send![view, enclosingScrollView];
        if !scroll.is_null() {
            let _: () = msg_send![scroll, scrollWheel: event];
            return;
        }

        let next: Id = msg_send![view, nextResponder];
        if !next.is_null() {
            let _: () = msg_send![next, scrollWheel: event];
        }
    }
}

/// Add a view to NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` and `view` a valid `NSView`.
pub unsafe fn stack_view_add(stack: Id, view: Id) {
    unsafe {
        // NSStackView uses Auto Layout for arranged subviews. Our views are created with manual
        // frames, so we need to:
        // - opt out of autoresizing-mask constraints
        // - provide at least a height constraint, otherwise subviews can collapse/overlap
        let _: () = msg_send![view, setTranslatesAutoresizingMaskIntoConstraints: false];

        let _: () = msg_send![stack, addArrangedSubview: view];

        // Ensure a deterministic width. Without leading/trailing constraints, NSStackView can
        // produce ambiguous layouts (overlaps / broken scrolling) when used as a scroll document.
        let view_leading: Id = msg_send![view, leadingAnchor];
        let view_trailing: Id = msg_send![view, trailingAnchor];
        let stack_leading: Id = msg_send![stack, leadingAnchor];
        let stack_trailing: Id = msg_send![stack, trailingAnchor];
        if !view_leading.is_null()
            && !view_trailing.is_null()
            && !stack_leading.is_null()
            && !stack_trailing.is_null()
        {
            let leading: Id = msg_send![view_leading, constraintEqualToAnchor: stack_leading];
            let trailing: Id = msg_send![view_trailing, constraintEqualToAnchor: stack_trailing];
            if !leading.is_null() {
                let _: () = msg_send![leading, setActive: true];
            }
            if !trailing.is_null() {
                let _: () = msg_send![trailing, setActive: true];
            }
        }

        // Pin height to the initial frame height (good enough for our chat bubbles/cards).
        let frame: CGRect = msg_send![view, frame];
        let height_anchor: Id = msg_send![view, heightAnchor];
        let height_constraint: Id =
            msg_send![height_anchor, constraintEqualToConstant: frame.size.height];
        // Tag for later updates (streaming bubbles grow).
        let _: () = msg_send![height_constraint, setIdentifier: ns_string("codescribe_height")];
        let _: () = msg_send![height_constraint, setActive: true];
    }
}

/// Remove all views from NSStackView
/// # Safety
/// `stack` must be a valid `NSStackView` instance.
pub unsafe fn stack_view_clear(stack: Id) {
    unsafe {
        let arranged: Id = msg_send![stack, arrangedSubviews];
        let count: usize = msg_send![arranged, count];

        for i in (0..count).rev() {
            let view: Id = msg_send![arranged, objectAtIndex: i];
            // For NSStackView, removing an arranged subview requires two steps:
            // 1) removeArrangedSubview (removes constraints/arrangement bookkeeping)
            // 2) removeFromSuperview (removes it from the view hierarchy)
            let _: () = msg_send![stack, removeArrangedSubview: view];
            let _: () = msg_send![view, removeFromSuperview];
        }
    }
}

// ============================================================================
// Editable Text Input Helpers
// ============================================================================

/// Create an editable text input field with a border and placeholder.
pub fn create_text_input(frame: CGRect, placeholder: &str, initial_value: &str) -> Id {
    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_text_field, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];

        let _: () = msg_send![field, setBezeled: true];
        let _: () = msg_send![field, setEditable: true];
        let _: () = msg_send![field, setSelectable: true];
        let _: () = msg_send![field, setDrawsBackground: true];

        let font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
        let _: () = msg_send![field, setFont: font];

        let ph = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: ph];

        if !initial_value.is_empty() {
            let val = ns_string(initial_value);
            let _: () = msg_send![field, setStringValue: val];
        }

        field
    }
}

/// Create a secure (password) text input field.
pub fn create_secure_text_input(frame: CGRect, placeholder: &str) -> Id {
    unsafe {
        let ns_secure = Class::get("NSSecureTextField").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        let field: Id = msg_send![ns_secure, alloc];
        let field: Id = msg_send![field, initWithFrame: frame];

        let _: () = msg_send![field, setBezeled: true];
        let _: () = msg_send![field, setEditable: true];
        let _: () = msg_send![field, setSelectable: true];
        let _: () = msg_send![field, setDrawsBackground: true];

        let font: Id = msg_send![ns_font, systemFontOfSize: 13.0f64];
        let _: () = msg_send![field, setFont: font];

        let ph = ns_string(placeholder);
        let _: () = msg_send![field, setPlaceholderString: ph];

        field
    }
}

/// Create an NSSlider (continuous, horizontal).
pub fn create_slider(frame: CGRect, min: f64, max: f64, value: f64) -> Id {
    unsafe {
        let ns_slider = Class::get("NSSlider").unwrap();

        let slider: Id = msg_send![ns_slider, alloc];
        let slider: Id = msg_send![slider, initWithFrame: frame];

        let _: () = msg_send![slider, setMinValue: min];
        let _: () = msg_send![slider, setMaxValue: max];
        let _: () = msg_send![slider, setDoubleValue: value];
        // Fire action only on mouse-up to avoid spamming settings.json writes.
        let _: () = msg_send![slider, setContinuous: false];

        slider
    }
}
