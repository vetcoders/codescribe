//! Live-preview text filtering: decision-mode passthrough versus
//! word-boundary-stabilized streaming preview.

use super::state::{TranscriptionOverlayState, action_text_for_contract};

pub(super) fn display_text_for_state(state: &TranscriptionOverlayState) -> String {
    let text = if state.accumulated_text.trim().is_empty() {
        action_text_for_contract(state)
    } else {
        state.accumulated_text.clone()
    };
    overlay_visible_text(&text, state.decision_mode).to_string()
}

pub(super) fn overlay_visible_text(text: &str, decision_mode: bool) -> &str {
    if decision_mode || !overlay_live_preview_uses_stable_text() {
        // Decision mode must show exact contract payload without preview filtering.
        text
    } else {
        // Live preview shows only complete word boundaries to avoid jittery partial tails.
        stable_overlay_preview_text(text)
    }
}

fn overlay_live_preview_uses_stable_text() -> bool {
    std::env::var("CODESCRIBE_OVERLAY_STABLE_PREVIEW")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(super) fn stable_overlay_preview_text(text: &str) -> &str {
    if text.is_empty() {
        return text;
    }

    let ends_stable = text
        .chars()
        .last()
        .map(is_preview_boundary_char)
        .unwrap_or(false);
    if ends_stable {
        return text;
    }

    let mut last_boundary_idx = None;
    for (idx, ch) in text.char_indices() {
        if is_preview_boundary_char(ch) {
            last_boundary_idx = Some(idx + ch.len_utf8());
        }
    }

    match last_boundary_idx {
        Some(idx) => &text[..idx],
        None => text,
    }
}

fn is_preview_boundary_char(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | '('
                | ']'
                | '['
                | '}'
                | '{'
                | '"'
                | '\''
                | '…'
                | '—'
                | '-'
        )
}
