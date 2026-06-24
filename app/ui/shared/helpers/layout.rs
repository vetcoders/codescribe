use super::ui_tokens;

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
