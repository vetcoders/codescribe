//! Settings row and tab-layout builders: toggle rows, slider rows, scroll wrapping.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct ToggleRowSpec<'a> {
    pub(super) title: &'a str,
    pub(super) checked: bool,
    pub(super) action: objc::runtime::Sel,
    pub(super) description: Option<&'a str>,
    pub(super) tag: Option<isize>,
    pub(super) gap: f64,
}

#[derive(Clone, Copy)]
pub(super) struct SliderSettingRowSpec<'a> {
    pub(super) title: &'a str,
    pub(super) value_text: &'a str,
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) current: f64,
    pub(super) action: objc::runtime::Sel,
    pub(super) gap: f64,
}

#[derive(Clone, Copy)]
pub(super) struct SliderSettingRowHandles {
    pub(super) label: usize,
    pub(super) value_label: usize,
    pub(super) slider: usize,
}

impl SliderSettingRowHandles {
    pub(super) fn all_views(self) -> [usize; 3] {
        [self.label, self.value_label, self.slider]
    }
}

pub(super) fn toggle_row_step(has_description: bool, gap: f64) -> f64 {
    if has_description {
        TOGGLE_ROW_DESC_OFFSET + TOGGLE_ROW_DESC_HEIGHT + gap
    } else {
        TOGGLE_ROW_HEIGHT + gap
    }
}

pub(super) unsafe fn style_paper_input(field: Id) {
    let _: () = msg_send![field, setDrawsBackground: true];
    let input_bg = unsafe { settings_input_paper_bg() };
    let _: () = msg_send![field, setBackgroundColor: input_bg];
}

pub(super) unsafe fn settings_input_paper_bg() -> Id {
    let base = ui_colors::surface_paper_warm();
    msg_send![base, colorWithAlphaComponent: 0.84f64]
}

pub(super) unsafe fn add_tafla_header_separator(container: Id, x: f64, y: f64, width: f64) -> f64 {
    let separator = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(x, y), &CGSize::new(width, 1.0)),
        text: String::new(),
        background_color: Some(ui_colors::header_border()),
        ..Default::default()
    });
    let _: () = msg_send![separator, setAlphaValue: 0.9f64];
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        add_subview(container, separator);
    }
    y - 1.0
}

pub(super) unsafe fn add_settings_group_card(
    container: Id,
    x: f64,
    top_y: f64,
    width: f64,
    height: f64,
) -> Id {
    unsafe { build_settings_group_card(container, x, top_y, width, height.max(44.0), false) }
}

/// Padding (px) the dynamic card keeps below a section's last content row.
pub(super) const SETTINGS_CARD_BOTTOM_PAD: f64 = 12.0;

/// Draw a settings group card sized to the section's *actual* content advance
/// instead of a hand-tuned fixed height, inserted BEHIND the rows that were
/// already added.
///
/// AppKit has no constraint solver here: section rows are laid out by a manual
/// top-down `y` cursor. Hardcoding the card height (the old `172.0`/`170.0`/...)
/// drifts the instant a row step or token gap changes, so the last rows spill
/// past the card and collide with the next section. Capturing `top_y` before
/// the rows and `content_bottom_y` after them makes the card track reality and
/// stay collision-free at any font scale / value length.
///
/// `content_bottom_y` is the cursor position just below the last row; the card
/// extends `SETTINGS_CARD_BOTTOM_PAD` further down for breathing room. Inserted
/// at the back of `container` (NSWindowBelow) so the rows stay legible on top.
pub(super) unsafe fn add_settings_group_card_dynamic(
    container: Id,
    x: f64,
    top_y: f64,
    width: f64,
    content_bottom_y: f64,
) -> Id {
    let height = (top_y - (content_bottom_y - SETTINGS_CARD_BOTTOM_PAD)).max(44.0);
    unsafe { build_settings_group_card(container, x, top_y, width, height, true) }
}

unsafe fn build_settings_group_card(
    container: Id,
    x: f64,
    top_y: f64,
    width: f64,
    height: f64,
    behind: bool,
) -> Id {
    let ns_view = objc_class("NSView");
    let card: Id = msg_send![ns_view, alloc];
    let card: Id = msg_send![
        card,
        initWithFrame: CGRect::new(
            &CGPoint::new(x, top_y - height),
            &CGSize::new(width.max(120.0), height.max(44.0)),
        )
    ];
    let _: () = msg_send![card, setWantsLayer: true];
    let layer: Id = msg_send![card, layer];
    if !layer.is_null() {
        let bg = ui_colors::card_bg();
        let cg_color: Id = msg_send![bg, CGColor];
        let _: () = msg_send![layer, setBackgroundColor: cg_color];
        // SAFETY: `layer` belongs to the AppKit card view allocated above on the main thread.
        unsafe {
            crate::ui_helpers::apply_tafla_surface(layer, true);
        }
        let _: () = msg_send![layer, setMasksToBounds: true];
        let _: () = msg_send![layer, setShadowRadius: 0.0f64];
    }
    if behind {
        // NSWindowBelow == -1: drop the card to the back so the section's rows,
        // which were added first, render on top of it.
        let below: isize = -1;
        let nil_view: Id = std::ptr::null_mut();
        let _: () = msg_send![container, addSubview: card positioned: below relativeTo: nil_view];
    } else {
        // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
        unsafe {
            add_subview(container, card);
        }
    }
    card
}

pub(super) unsafe fn add_slider_setting_row(
    container: Id,
    action_handler: Id,
    x: f64,
    y: &mut f64,
    width: f64,
    secondary: Id,
    spec: SliderSettingRowSpec<'_>,
) -> SliderSettingRowHandles {
    let label = create_label(LabelConfig {
        frame: CGRect::new(&CGPoint::new(x, *y), &CGSize::new(136.0, 18.0)),
        text: spec.title.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: secondary,
        ..Default::default()
    });
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        add_subview(container, label);
    }

    let value_label = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(x + width - 110.0, *y),
            &CGSize::new(110.0, 18.0),
        ),
        text: spec.value_text.to_string(),
        font_size: ui_tokens::SMALL_FONT_SIZE,
        text_color: secondary,
        ..Default::default()
    });
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        add_subview(container, value_label);
    }

    let slider = create_slider(
        CGRect::new(
            &CGPoint::new(x + 140.0, *y - 1.0),
            &CGSize::new((width - 254.0).max(160.0), 20.0),
        ),
        spec.min,
        spec.max,
        spec.current,
    );
    let _: () = msg_send![slider, setContinuous: true];
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        button_set_action(slider, action_handler, spec.action);
        add_subview(container, slider);
    }

    *y -= 24.0 + spec.gap;
    SliderSettingRowHandles {
        label: label as usize,
        value_label: value_label as usize,
        slider: slider as usize,
    }
}

pub(super) unsafe fn autosize_tab_document_view(document_view: Id, minimum_height: f64) -> f64 {
    let subviews: Id = msg_send![document_view, subviews];
    if subviews.is_null() {
        let mut doc_frame: CGRect = msg_send![document_view, frame];
        doc_frame.origin = CGPoint::new(0.0, 0.0);
        doc_frame.size.height = minimum_height.max(doc_frame.size.height);
        let _: () = msg_send![document_view, setFrame: doc_frame];
        return doc_frame.size.height;
    }

    let count: usize = msg_send![subviews, count];
    if count == 0 {
        let mut doc_frame: CGRect = msg_send![document_view, frame];
        doc_frame.origin = CGPoint::new(0.0, 0.0);
        doc_frame.size.height = minimum_height.max(doc_frame.size.height);
        let _: () = msg_send![document_view, setFrame: doc_frame];
        return doc_frame.size.height;
    }

    let mut min_y = f64::INFINITY;
    let mut max_y = 0.0_f64;
    for idx in 0..count {
        let subview: Id = msg_send![subviews, objectAtIndex: idx];
        if subview.is_null() {
            continue;
        }
        let frame: CGRect = msg_send![subview, frame];
        min_y = min_y.min(frame.origin.y);
        max_y = max_y.max(frame.origin.y + frame.size.height);
    }

    let shift_y = if min_y.is_finite() && min_y < SETTINGS_CONTENT_INSET_Y {
        SETTINGS_CONTENT_INSET_Y - min_y
    } else {
        0.0
    };

    if shift_y > 0.0 {
        for idx in 0..count {
            let subview: Id = msg_send![subviews, objectAtIndex: idx];
            if subview.is_null() {
                continue;
            }
            let mut frame: CGRect = msg_send![subview, frame];
            frame.origin.y += shift_y;
            let _: () = msg_send![subview, setFrame: frame];
        }
        max_y += shift_y;
    }

    let mut doc_frame: CGRect = msg_send![document_view, frame];
    doc_frame.origin = CGPoint::new(0.0, 0.0);
    doc_frame.size.height = minimum_height.max(max_y.ceil());
    let _: () = msg_send![document_view, setFrame: doc_frame];
    doc_frame.size.height
}

pub(super) unsafe fn wrap_tab_content_in_scroll_view(frame: CGRect, document_view: Id) -> Id {
    let ns_scroll_view = objc_class("NSScrollView");
    let scroll: Id = msg_send![ns_scroll_view, alloc];
    let scroll: Id = msg_send![scroll, initWithFrame: frame];
    let _: () = msg_send![scroll, setHasVerticalScroller: true];
    let _: () = msg_send![scroll, setHasHorizontalScroller: false];
    let _: () = msg_send![scroll, setAutohidesScrollers: true];
    let _: () = msg_send![scroll, setBorderType: 0_isize]; // NSNoBorder
    let _: () = msg_send![scroll, setDrawsBackground: false];
    let _: () = msg_send![
        scroll,
        setAutoresizingMask: 2_isize | 16_isize // width + height
    ];

    let doc_h = unsafe { autosize_tab_document_view(document_view, frame.size.height) };
    let _: () = msg_send![scroll, setDocumentView: document_view];
    let _: () = msg_send![scroll, setHasVerticalScroller: doc_h > frame.size.height + 1.0];

    let clip_view: Id = msg_send![scroll, contentView];
    if !clip_view.is_null() {
        let top_point = CGPoint::new(0.0, (doc_h - frame.size.height).max(0.0));
        let _: () = msg_send![clip_view, scrollToPoint: top_point];
        let _: () = msg_send![scroll, reflectScrolledClipView: clip_view];
    }

    scroll
}

pub(super) unsafe fn add_toggle_row(
    container: Id,
    action_handler: Id,
    x: f64,
    y: &mut f64,
    width: f64,
    secondary: Id,
    spec: ToggleRowSpec<'_>,
) -> Id {
    let text_width = (width - TOGGLE_SWITCH_WIDTH - 10.0).max(80.0);
    let title_label = create_label(LabelConfig {
        frame: CGRect::new(
            &CGPoint::new(x, *y + 1.0),
            &CGSize::new(text_width, TOGGLE_ROW_HEIGHT),
        ),
        text: spec.title.to_string(),
        font_size: ui_tokens::BODY_FONT_SIZE,
        text_color: crate::ui_helpers::color_label(),
        ..Default::default()
    });
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        add_subview(container, title_label);
    }

    let toggle = create_toggle(
        CGRect::new(
            &CGPoint::new(x + width - TOGGLE_SWITCH_WIDTH, *y),
            &CGSize::new(TOGGLE_SWITCH_WIDTH, TOGGLE_SWITCH_HEIGHT),
        ),
        spec.checked,
    );
    if let Some(tag) = spec.tag {
        let _: () = msg_send![toggle, setTag: tag];
    }
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        button_set_action(toggle, action_handler, spec.action);
        add_subview(container, toggle);
    }

    if let Some(desc) = spec.description {
        let desc_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(x + TOGGLE_ROW_LABEL_INDENT, *y - TOGGLE_ROW_DESC_OFFSET),
                &CGSize::new(
                    (width - TOGGLE_ROW_LABEL_INDENT - TOGGLE_SWITCH_WIDTH - 10.0).max(60.0),
                    TOGGLE_ROW_DESC_HEIGHT,
                ),
            ),
            text: desc.to_string(),
            font_size: ui_tokens::MICRO_FONT_SIZE,
            text_color: secondary,
            ..Default::default()
        });
        // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
        unsafe {
            add_subview(container, desc_label);
        }
    }

    *y -= toggle_row_step(spec.description.is_some(), spec.gap);
    toggle
}
