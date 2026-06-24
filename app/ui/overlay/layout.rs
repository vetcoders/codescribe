//! Geometry constants, layout metrics, and the unlocked window-resize path.
//!
//! `resize_overlay_unlocked` is the single place that reflows the overlay
//! window and all its chrome (header, status, hint, spinner, blur, buttons)
//! to fit the current text content. Call ONLY outside the `OVERLAY_STATE`
//! lock — see `state::OverlaySnapshot` for the deadlock rationale.

use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::{msg_send, sel, sel_impl};

use super::state::OverlaySnapshot;
use crate::ui_helpers::Id;

// Window level constants
pub(super) const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

pub(super) const OVERLAY_WINDOW_WIDTH: f64 = 420.0;
pub(super) const OVERLAY_WINDOW_MIN_WIDTH: f64 = 360.0;
pub(super) const OVERLAY_WINDOW_MAX_WIDTH: f64 = 760.0;
pub(super) const OVERLAY_WINDOW_MIN_HEIGHT: f64 = 180.0;
pub(super) const OVERLAY_WINDOW_MAX_HEIGHT_RATIO: f64 = 0.5;
pub(super) const OVERLAY_PADDING: f64 = 16.0;
pub(super) const OVERLAY_HEADER_HEIGHT: f64 = 20.0;
pub(super) const OVERLAY_STATUS_HEIGHT: f64 = 20.0;
pub(super) const OVERLAY_INFO_HEIGHT: f64 = 12.0;
pub(super) const OVERLAY_STATUS_WIDTH: f64 = 100.0;
pub(super) const OVERLAY_HEADER_GAP: f64 = 4.0;
pub(super) const OVERLAY_CONTENT_GAP: f64 = 8.0;
pub(super) const OVERLAY_TEXT_MIN_HEIGHT: f64 = 44.0;
pub(super) const OVERLAY_BUTTON_HEIGHT: f64 = 28.0;
pub(super) const OVERLAY_BUTTON_MARGIN: f64 = 8.0;

pub(super) const OVERLAY_LAYOUT_THROTTLE_MS: u64 = 80;
pub(super) const OVERLAY_LAYOUT_HYSTERESIS_PX: f64 = 1.0;
pub(super) const NSVIEW_WIDTH_SIZABLE: isize = 2;
pub(super) const NSVIEW_MAX_X_MARGIN: isize = 4;
pub(super) const NSVIEW_MIN_Y_MARGIN: isize = 8;
pub(super) const NSVIEW_HEIGHT_SIZABLE: isize = 16;
pub(super) const NSVIEW_MAX_Y_MARGIN: isize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRange {
    location: usize,
    length: usize,
}

pub(super) fn overlay_top_reserved_height() -> f64 {
    OVERLAY_PADDING
        + OVERLAY_HEADER_HEIGHT
        + OVERLAY_HEADER_GAP
        + OVERLAY_INFO_HEIGHT
        + OVERLAY_CONTENT_GAP
}

pub(super) fn overlay_bottom_reserved_height() -> f64 {
    OVERLAY_PADDING + OVERLAY_BUTTON_HEIGHT + OVERLAY_BUTTON_MARGIN
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OverlayLayoutMetrics {
    pub(super) target_height: f64,
    pub(super) text_viewport_height: f64,
    pub(super) text_document_height: f64,
    pub(super) needs_scroll: bool,
}

pub(super) fn compute_overlay_layout_metrics(
    text_content_height: f64,
    min_height: f64,
    max_height: f64,
) -> OverlayLayoutMetrics {
    let clamped_content_height = text_content_height.max(OVERLAY_TEXT_MIN_HEIGHT);
    let chrome_height = overlay_top_reserved_height() + overlay_bottom_reserved_height();
    let required_window_height = clamped_content_height + chrome_height;
    let target_height = required_window_height.max(min_height).min(max_height);
    let text_viewport_height = (target_height - chrome_height).max(OVERLAY_TEXT_MIN_HEIGHT);
    let text_document_height = clamped_content_height.max(text_viewport_height);
    let needs_scroll = text_document_height > text_viewport_height + 0.5;

    OverlayLayoutMetrics {
        target_height,
        text_viewport_height,
        text_document_height,
        needs_scroll,
    }
}

pub(super) fn measure_text_view_content_height(text_view: Id, width: f64) -> f64 {
    unsafe {
        let layout: Id = msg_send![text_view, layoutManager];
        let container: Id = msg_send![text_view, textContainer];
        if layout.is_null() || container.is_null() {
            return 0.0;
        }
        let _: () = msg_send![container, setContainerSize: CGSize::new(width.max(1.0), f64::MAX)];
        let _: () = msg_send![layout, ensureLayoutForTextContainer: container];
        let used_rect: CGRect = msg_send![layout, usedRectForTextContainer: container];
        used_rect.size.height.max(0.0)
    }
}

pub(super) fn scroll_text_view_to_bottom(text_view: Id) {
    unsafe {
        let text: Id = msg_send![text_view, string];
        if text.is_null() {
            return;
        }
        let len: usize = msg_send![text, length];
        if len == 0 {
            return;
        }
        let range = NSRange {
            location: len,
            length: 0,
        };
        let _: () = msg_send![text_view, scrollRangeToVisible: range];
    }
}

/// Resize overlay window to fit text content. Call ONLY outside the `OVERLAY_STATE` lock.
/// Returns the new `last_applied_height` for write-back to state.
pub(super) fn resize_overlay_unlocked(snap: &OverlaySnapshot) -> f64 {
    let (window_ptr, text_scroll_ptr, text_view_ptr) =
        match (snap.window, snap.text_scroll_view, snap.text_view) {
            (Some(w), Some(ts), Some(tv)) => (w as Id, ts as Id, tv as Id),
            _ => return snap.last_applied_height,
        };

    unsafe {
        let current_frame: CGRect = msg_send![window_ptr, frame];
        let window_width = current_frame
            .size
            .width
            .clamp(OVERLAY_WINDOW_MIN_WIDTH, OVERLAY_WINDOW_MAX_WIDTH);
        let text_width = (window_width - OVERLAY_PADDING * 2.0).max(120.0);
        let text_content_height = measure_text_view_content_height(text_view_ptr, text_width);
        let metrics =
            compute_overlay_layout_metrics(text_content_height, snap.min_height, snap.max_height);
        let top_y = current_frame.origin.y + current_frame.size.height;
        let should_resize =
            (snap.last_applied_height - metrics.target_height).abs() > OVERLAY_LAYOUT_HYSTERESIS_PX;
        let applied_height = if should_resize {
            let new_frame = CGRect {
                origin: CGPoint {
                    x: current_frame.origin.x,
                    y: top_y - metrics.target_height,
                },
                size: CGSize {
                    width: window_width,
                    height: metrics.target_height,
                },
            };
            let _: () = msg_send![window_ptr, setFrame: new_frame display: true];
            metrics.target_height
        } else {
            current_frame.size.height
        };
        let _: () = msg_send![window_ptr, setLevel: NS_FLOATING_WINDOW_LEVEL];

        let text_frame = CGRect {
            origin: CGPoint {
                x: OVERLAY_PADDING,
                y: overlay_bottom_reserved_height(),
            },
            size: CGSize {
                width: text_width,
                height: metrics.text_viewport_height,
            },
        };
        let _: () = msg_send![text_scroll_ptr, setFrame: text_frame];

        let document_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: text_width,
                height: metrics.text_document_height,
            },
        };
        let _: () = msg_send![text_view_ptr, setFrame: document_frame];
        let _: () =
            msg_send![text_view_ptr, setMinSize: CGSize::new(0.0, metrics.text_viewport_height)];
        let _: () = msg_send![text_scroll_ptr, setHasVerticalScroller: metrics.needs_scroll];
        if metrics.needs_scroll {
            scroll_text_view_to_bottom(text_view_ptr);
        }

        let header_y = applied_height - OVERLAY_PADDING - OVERLAY_HEADER_HEIGHT;
        let info_y = header_y - OVERLAY_HEADER_GAP - OVERLAY_INFO_HEIGHT;
        let spinner_size = 14.0;
        let spinner_x = window_width - OVERLAY_PADDING - spinner_size;
        let status_gap = 6.0;
        let status_max_x = spinner_x - status_gap;
        let status_width = OVERLAY_STATUS_WIDTH.min((status_max_x - OVERLAY_PADDING).max(80.0));
        let status_x = (status_max_x - status_width).max(OVERLAY_PADDING);
        let header_width = (status_x - OVERLAY_CONTENT_GAP - OVERLAY_PADDING).max(120.0);

        if let Some(header_ptr) = snap.header_label {
            let header_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: header_y,
                },
                size: CGSize {
                    width: header_width,
                    height: OVERLAY_HEADER_HEIGHT,
                },
            };
            let _: () = msg_send![header_ptr as Id, setFrame: header_frame];
        }

        if let Some(status_ptr) = snap.status_field {
            let status_frame = CGRect {
                origin: CGPoint {
                    x: status_x,
                    y: header_y,
                },
                size: CGSize {
                    width: status_width,
                    height: OVERLAY_STATUS_HEIGHT,
                },
            };
            let _: () = msg_send![status_ptr as Id, setFrame: status_frame];
        }

        if let Some(auto_hide_ptr) = snap.auto_hide_label {
            let hint_frame = CGRect {
                origin: CGPoint {
                    x: OVERLAY_PADDING,
                    y: info_y,
                },
                size: CGSize {
                    width: window_width - OVERLAY_PADDING * 2.0,
                    height: OVERLAY_INFO_HEIGHT,
                },
            };
            let _: () = msg_send![auto_hide_ptr as Id, setFrame: hint_frame];
        }

        if let Some(spinner_ptr) = snap.progress_indicator {
            let spinner_frame = CGRect {
                origin: CGPoint {
                    x: spinner_x,
                    y: header_y + ((OVERLAY_HEADER_HEIGHT - spinner_size) / 2.0).max(0.0),
                },
                size: CGSize {
                    width: spinner_size,
                    height: spinner_size,
                },
            };
            let _: () = msg_send![spinner_ptr as Id, setFrame: spinner_frame];
        }

        if let Some(blur_ptr) = snap.blur_view {
            let blur_frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: window_width,
                    height: applied_height,
                },
            };
            let _: () = msg_send![blur_ptr as Id, setFrame: blur_frame];
        }

        let button_width = 100.0;
        let button_gap = 10.0;
        let row_width = button_width * 3.0 + button_gap * 2.0;
        let row_x = (window_width - row_width) / 2.0;
        let save_frame = CGRect {
            origin: CGPoint {
                x: row_x,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let copy_frame = CGRect {
            origin: CGPoint {
                x: row_x + button_width + button_gap,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };
        let augment_frame = CGRect {
            origin: CGPoint {
                x: row_x + (button_width + button_gap) * 2.0,
                y: OVERLAY_PADDING,
            },
            size: CGSize {
                width: button_width,
                height: OVERLAY_BUTTON_HEIGHT,
            },
        };

        if let Some(save_ptr) = snap.save_button {
            let _: () = msg_send![save_ptr as Id, setFrame: save_frame];
        }
        if let Some(copy_ptr) = snap.copy_button {
            let _: () = msg_send![copy_ptr as Id, setFrame: copy_frame];
        }
        if let Some(augment_ptr) = snap.augment_button {
            let _: () = msg_send![augment_ptr as Id, setFrame: augment_frame];
        }
        if let Some(commit_ptr) = snap.commit_button {
            let commit_frame = CGRect {
                origin: CGPoint {
                    x: (window_width - button_width) / 2.0,
                    y: OVERLAY_PADDING,
                },
                size: CGSize {
                    width: button_width,
                    height: OVERLAY_BUTTON_HEIGHT,
                },
            };
            let _: () = msg_send![commit_ptr as Id, setFrame: commit_frame];
        }

        applied_height
    }
}
