//! Tab switching and header/footer control reflow.

use super::*;

/// Switch to Agent tab programmatically
pub fn show_agent_tab() {
    Queue::main().exec_async(|| {
        // If the overlay isn't created yet, defer tab selection until build completes.
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.window.is_none() {
            state.pending_tab = Some(Tab::Agent);
            state.active_tab = Tab::Agent;
            return;
        }
        drop(state);
        update_active_tab_impl(Tab::Agent);
    });
}

/// Switch to Drawer tab programmatically
pub fn show_drawer_tab() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.window.is_none() {
            state.pending_tab = Some(Tab::Drawer);
            state.active_tab = Tab::Drawer;
            return;
        }
        drop(state);
        update_active_tab_impl(Tab::Drawer);
    });
}

// ═══════════════════════════════════════════════════════════
// Internal Implementation Functions
// ═══════════════════════════════════════════════════════════

pub fn update_active_tab_impl(tab: Tab) {
    // DEADLOCK PREVENTION: extract widget pointers under lock, drop lock before
    // AppKit calls (setCollapsed can animate and spin a nested run-loop).
    let (
        _prev_tab,
        tab_drawer_btn,
        tab_agent_btn,
        tab_settings_btn,
        sidebar_item,
        content_item,
        split_vc,
        drawer_sv,
        search_f,
        search_l,
        fav_btn,
        drawer_edge,
        agent_sv,
        agent_bar,
        agent_attach,
        agent_send,
        title_label,
        window_ptr,
        agent_input_tv,
        need_chat_update,
    ) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let prev = state.active_tab;
        state.active_tab = tab;
        (
            prev, // kept to compute need_chat_update below
            state.tab_drawer_button,
            state.tab_agent_button,
            state.tab_settings_button,
            state.split_sidebar_item,
            state.split_content_item,
            state.split_view_controller,
            state.drawer_scroll_view,
            state.search_field,
            state.search_label,
            state.favorites_button,
            state.drawer_edge_effect,
            state.agent_scroll_view,
            state.agent_input_bar,
            state.agent_attach_button,
            state.agent_send_button,
            state.title_label,
            state.window,
            state.agent_input_text_view,
            tab == Tab::Agent && prev != Tab::Agent,
        )
    }; // Lock dropped.

    let show_drawer = tab == Tab::Drawer;
    let show_agent = tab == Tab::Agent;

    unsafe {
        if let Some(b) = tab_drawer_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, show_drawer);
        }
        if let Some(b) = tab_agent_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, show_agent);
        }
        if let Some(b) = tab_settings_btn {
            crate::ui_helpers::set_tab_button_active(b as Id, false);
        }
        if let Some(p) = sidebar_item {
            let _: () = msg_send![p as Id, setCollapsed: show_agent];
        }
        if let Some(p) = content_item {
            let _: () = msg_send![p as Id, setCollapsed: !show_agent];
        }
        if let Some(p) = split_vc {
            let split_view: Id = msg_send![p as Id, view];
            if !split_view.is_null() {
                crate::ui_helpers::set_hidden(split_view, false);
            }
        }
        if let Some(p) = drawer_sv {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = search_f {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = search_l {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = fav_btn {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = drawer_edge {
            crate::ui_helpers::set_hidden(p as Id, !show_drawer);
        }
        if let Some(p) = agent_sv {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_bar {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_attach {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = agent_send {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }
        if let Some(p) = title_label {
            crate::ui_helpers::set_hidden(p as Id, !show_agent);
        }

        // Complex agent-tab operations need full state access; re-lock briefly.
        if show_agent {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if need_chat_update {
                update_chat_view_with_state(&mut state, true);
            }
            resize_agent_input_locked(&mut state);
        }

        // Nudge first responder to agent input when window is already key.
        if tab == Tab::Agent
            && let (Some(w), Some(inp)) = (window_ptr, agent_input_tv)
        {
            let window = w as Id;
            let is_key: bool = msg_send![window, isKeyWindow];
            if is_key {
                let _: bool = msg_send![window, makeFirstResponder: inp as Id];
            }
        }
    }
}

pub fn update_active_tab_locked(state: &mut VoiceChatOverlayState, tab: Tab) {
    unsafe {
        let prev_tab = state.active_tab;
        state.active_tab = tab;

        if let Some(button) = state.tab_drawer_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Drawer);
        }
        if let Some(button) = state.tab_agent_button {
            crate::ui_helpers::set_tab_button_active(button as Id, tab == Tab::Agent);
        }
        if let Some(button) = state.tab_settings_button {
            crate::ui_helpers::set_tab_button_active(button as Id, false);
        }

        let show_drawer = tab == Tab::Drawer;
        let show_agent = tab == Tab::Agent;

        if let Some(sidebar_item) = state.split_sidebar_item {
            let item = sidebar_item as Id;
            let _: () = msg_send![item, setCollapsed: show_agent];
        }
        if let Some(content_item) = state.split_content_item {
            let item = content_item as Id;
            let _: () = msg_send![item, setCollapsed: !show_agent];
        }
        if let Some(split_controller) = state.split_view_controller {
            let split_view: Id = msg_send![split_controller as Id, view];
            if !split_view.is_null() {
                crate::ui_helpers::set_hidden(split_view, false);
            }
        }
        if let Some(drawer_view) = state.drawer_scroll_view {
            crate::ui_helpers::set_hidden(drawer_view as Id, !show_drawer);
        }
        if let Some(search_field) = state.search_field {
            crate::ui_helpers::set_hidden(search_field as Id, !show_drawer);
        }
        if let Some(search_label) = state.search_label {
            crate::ui_helpers::set_hidden(search_label as Id, !show_drawer);
        }
        if let Some(favorites_button) = state.favorites_button {
            crate::ui_helpers::set_hidden(favorites_button as Id, !show_drawer);
        }
        if let Some(edge) = state.drawer_edge_effect {
            crate::ui_helpers::set_hidden(edge as Id, !show_drawer);
        }
        if let Some(agent_view) = state.agent_scroll_view {
            crate::ui_helpers::set_hidden(agent_view as Id, !show_agent);
        }
        if let Some(agent_input_bar) = state.agent_input_bar {
            crate::ui_helpers::set_hidden(agent_input_bar as Id, !show_agent);
        }
        if let Some(agent_attach) = state.agent_attach_button {
            crate::ui_helpers::set_hidden(agent_attach as Id, !show_agent);
        }
        if let Some(agent_send) = state.agent_send_button {
            crate::ui_helpers::set_hidden(agent_send as Id, !show_agent);
        }
        if let Some(title_label) = state.title_label {
            crate::ui_helpers::set_hidden(title_label as Id, !show_agent);
        }

        if show_agent {
            // Populate the Agent view on tab switch so the empty-state CTA is visible.
            // Important: do this only on transition to Agent; `ensure_agent_tab_visible` can
            // call `update_active_tab_locked(Tab::Agent)` frequently during streaming.
            if prev_tab != Tab::Agent {
                update_chat_view_with_state(state, true);
            }
            resize_agent_input_locked(state);
        }

        // When switching to Agent, make sure the input field can actually receive text.
        // We do NOT force activation (to avoid stealing focus), but if the window is already
        // key, we nudge first responder to the input field for better UX.
        if tab == Tab::Agent
            && let (Some(window_ptr), Some(input_ptr)) = (state.window, state.agent_input_text_view)
        {
            let window = window_ptr as Id;
            let is_key: bool = msg_send![window, isKeyWindow];
            if is_key {
                let _: bool = msg_send![window, makeFirstResponder: input_ptr as Id];
            }
        }
    }
}

/// Reflow Agent layout after the overlay window was resized.
///
/// Without this, long messages can look clipped until the next message arrives.
pub fn reflow_agent_after_resize_impl() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
    if state.active_tab != Tab::Agent {
        return;
    }

    update_chat_view_with_state(&mut state, false);
    resize_agent_input_locked(&mut state);
}

/// Lightweight layout pass for window resizing (keeps inputs/footers aligned).
pub fn reflow_overlay_after_resize_impl() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
    reflow_header_controls_locked(&mut state);
    reflow_footer_controls_locked(&mut state);
    resize_agent_input_locked(&mut state);
}

pub fn reflow_header_controls_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let (
            Some(drawer_ptr),
            Some(agent_ptr),
            Some(settings_ptr),
            Some(favorites_ptr),
            Some(status_ptr),
        ) = (
            state.tab_drawer_button,
            state.tab_agent_button,
            state.tab_settings_button,
            state.favorites_button,
            state.status_pill,
        )
        else {
            return;
        };

        let tab_drawer_button = drawer_ptr as Id;
        let tab_agent_button = agent_ptr as Id;
        let tab_settings_button = settings_ptr as Id;
        let favorites_button = favorites_ptr as Id;
        let status_pill = status_ptr as Id;

        let favorites_frame: CGRect = msg_send![favorites_button, frame];
        let right_cluster_start_x = favorites_frame.origin.x
            - (ui_tokens::CHAT_HEADER_BUTTON_SIZE + ui_tokens::CHAT_HEADER_BUTTON_GAP);

        let header_safe_x = ui_tokens::TRAFFIC_LIGHTS_SPACER_WIDTH + 6.0;
        let layout = chat_header_layout(header_safe_x, 0.0, right_cluster_start_x);

        let drawer_frame: CGRect = msg_send![tab_drawer_button, frame];
        let tab_y = drawer_frame.origin.y;
        let tab_h = drawer_frame.size.height.max(20.0);
        let tab_w = layout.tab_button_width.max(0.0);
        let tab_gap = layout.tab_button_gap.max(0.0);

        let tab_drawer_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_drawer_button, setFrame: tab_drawer_frame];

        let tab_agent_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x + tab_w + tab_gap, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_agent_button, setFrame: tab_agent_frame];

        let tab_settings_frame = CGRect::new(
            &CGPoint::new(layout.tab_cluster_x + (tab_w + tab_gap) * 2.0, tab_y),
            &CGSize::new(tab_w, tab_h),
        );
        let _: () = msg_send![tab_settings_button, setFrame: tab_settings_frame];

        let status_h = ui_tokens::STATUS_PILL_HEIGHT;
        let status_y = (tab_y + (tab_h - status_h) * 0.5).max(0.0);
        let status_frame = CGRect::new(
            &CGPoint::new(layout.status_pill_x, status_y),
            &CGSize::new(layout.status_pill_width.max(0.0), status_h),
        );
        let _: () = msg_send![status_pill, setFrame: status_frame];
        let _: () = msg_send![status_pill, setHidden: !layout.show_status_pill];

        if let Some(dot_ptr) = state.status_pill_dot {
            let dot = dot_ptr as Id;
            let dot_size = ui_tokens::STATUS_DOT_SIZE;
            let dot_frame = CGRect::new(
                &CGPoint::new(
                    ui_tokens::STATUS_PILL_DOT_INSET_X,
                    (status_h - dot_size) * 0.5,
                ),
                &CGSize::new(dot_size, dot_size),
            );
            let _: () = msg_send![dot, setFrame: dot_frame];
        }

        if let Some(label_ptr) = state.status_pill_label {
            let label = label_ptr as Id;
            let label_width = (layout.status_pill_width
                - ui_tokens::STATUS_PILL_LABEL_INSET_X
                - ui_tokens::STATUS_PILL_LABEL_INSET_RIGHT)
                .max(0.0);
            let label_frame = CGRect::new(
                &CGPoint::new(ui_tokens::STATUS_PILL_LABEL_INSET_X, 1.0),
                &CGSize::new(label_width, (status_h - 2.0).max(0.0)),
            );
            let _: () = msg_send![label, setFrame: label_frame];
        }
    }
}

pub fn reflow_footer_controls_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(blur_ptr) = state.blur_view else {
            return;
        };
        let blur_view = blur_ptr as Id;
        let bounds: CGRect = msg_send![blur_view, bounds];
        let content_bounds = layout_region_frame_for_view(blur_view).unwrap_or(bounds);

        let footer_height = ui_tokens::FOOTER_HEIGHT;
        let footer_base_y = content_bounds.origin.y;
        let content_pad = ui_tokens::EDGE_PADDING;
        let search_x = content_bounds.origin.x + content_pad;
        let search_w = (content_bounds.size.width - content_pad * 2.0).max(160.0);

        if let Some(label_ptr) = state.search_label {
            let label = label_ptr as Id;
            let frame = CGRect::new(
                &CGPoint::new(search_x, footer_base_y + footer_height - 20.0),
                &CGSize::new(search_w, 16.0),
            );
            let _: () = msg_send![label, setFrame: frame];
        }

        if let Some(field_ptr) = state.search_field {
            let field = field_ptr as Id;
            let frame = CGRect::new(
                &CGPoint::new(search_x, footer_base_y + 12.0),
                &CGSize::new(search_w, 24.0),
            );
            let _: () = msg_send![field, setFrame: frame];
        }

        if let Some(label_ptr) = state.title_label {
            let label = label_ptr as Id;
            let label_w = ui_tokens::CHAT_TITLE_LABEL_WIDTH;
            let label_h = 16.0;
            let frame = CGRect::new(
                &CGPoint::new(
                    content_bounds.origin.x + content_bounds.size.width - content_pad - label_w,
                    footer_base_y + ((footer_height - label_h) / 2.0).max(4.0),
                ),
                &CGSize::new(label_w, label_h),
            );
            let _: () = msg_send![label, setFrame: frame];
        }

        let content_gap = ui_tokens::CONTENT_GAP;
        let content_frame = CGRect::new(
            &CGPoint::new(
                content_bounds.origin.x + content_pad,
                content_bounds.origin.y + footer_height + content_gap,
            ),
            &CGSize::new(
                (content_bounds.size.width - content_pad * 2.0).max(0.0),
                (content_bounds.size.height - footer_height - content_gap).max(0.0),
            ),
        );

        if let Some(split_controller) = state.split_view_controller {
            let split_view: Id = msg_send![split_controller as Id, view];
            if !split_view.is_null() {
                let _: () = msg_send![split_view, setFrame: content_frame];
            }
        }
    }
}
