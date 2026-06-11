//! Status pill, runtime-degraded banner, context summary and conversation state.

use super::*;

/// Update the status text in the overlay
pub fn update_voice_chat_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        run_when_overlay_unlocked(move || update_voice_chat_status_impl(&status_owned));
    });
}

/// Persist runtime degradation state used by status and tooltip rendering.
pub fn set_voice_chat_runtime_degraded(is_degraded: bool, reason: Option<&str>) {
    let reason_owned = reason.map(str::to_string);
    Queue::main().exec_async(move || {
        set_voice_chat_runtime_degraded_impl(is_degraded, reason_owned);
    });
}

/// Update the context summary shown in the overlay header (best-effort debug aid).
///
/// Examples:
/// - `ctx: Visual Studio Code | sel: 123`
/// - `ctx: Finder | sel: 0`
pub fn update_voice_chat_context_summary(summary: &str) {
    let summary_owned = summary.to_string();
    Queue::main().exec_async(move || {
        update_voice_chat_context_summary_impl(&summary_owned);
    });
}

/// Set the target app name to re-activate for paste actions.
///
/// This is best-effort and primarily used to paste assistant output back into
/// the app where the user was working before interacting with the overlay.
pub fn set_voice_chat_target_app(app_name: Option<String>) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.last_target_app = app_name;
    });
}

/// Update the conversation mode state (Moshi full-duplex indicators)
pub fn update_conversation_state(new_state: ConversationModeState) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.conversation_state = new_state;
        // Update status text based on conversation state
        let status = match new_state {
            ConversationModeState::Inactive => "Ready",
            ConversationModeState::Listening => "Listening...",
            ConversationModeState::UserSpeaking => "You're speaking...",
            ConversationModeState::Processing => "Processing...",
            ConversationModeState::AssistantSpeaking => "Moshi responding...",
            ConversationModeState::Interrupted => "Interrupted",
        };
        drop(state);
        update_voice_chat_status_impl(status);
    });
}

/// Check if conversation mode is active
pub fn is_conversation_active() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    !matches!(state.conversation_state, ConversationModeState::Inactive)
}

pub fn update_voice_chat_status_impl(status: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    apply_voice_chat_status(&mut state, status);
}

/// Apply status text + pill + tray update on an already-held `OVERLAY_STATE`
/// guard. Must NOT lock `OVERLAY_STATE`: callers that already hold the guard
/// (e.g. `apply_agent_thinking`) rely on this to avoid a re-entrant deadlock on
/// the non-reentrant `std::sync::Mutex`.
pub fn apply_voice_chat_status(state: &mut VoiceChatOverlayState, status: &str) {
    let trimmed = status.trim();
    state.status_base_text = if trimmed.is_empty() {
        "Ready".to_string()
    } else {
        trimmed.to_string()
    };
    state.status_text = compose_runtime_status_text(
        &state.status_base_text,
        state.is_agent_degraded,
        state.runtime_degraded_reason.as_deref(),
    );
    let next_kind = status_kind_for_runtime(&state.status_base_text, state.is_agent_degraded);
    state.status_kind = next_kind;
    apply_status_pill(state);
    let _ = crate::tray::update_tray_status(next_kind.to_tray());
}

pub fn set_voice_chat_runtime_degraded_impl(is_degraded: bool, reason: Option<String>) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.runtime_degraded = is_degraded;
    state.is_agent_degraded = is_degraded;
    state.runtime_degraded_reason = if is_degraded {
        reason.and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    } else {
        None
    };
    state.status_text = compose_runtime_status_text(
        &state.status_base_text,
        state.is_agent_degraded,
        state.runtime_degraded_reason.as_deref(),
    );
    state.status_kind = status_kind_for_runtime(&state.status_base_text, state.is_agent_degraded);
    apply_status_pill(&state);
    let _ = crate::tray::update_tray_status(state.status_kind.to_tray());
}

pub fn status_kind_for_runtime(base_status: &str, runtime_degraded: bool) -> UiStatus {
    if runtime_degraded {
        UiStatus::Error
    } else {
        status_from_detail(base_status)
    }
}

pub fn compose_runtime_status_text(
    base_status: &str,
    runtime_degraded: bool,
    reason: Option<&str>,
) -> String {
    let base = base_status.trim();
    if !runtime_degraded {
        if base.is_empty() {
            "Ready".to_string()
        } else {
            base.to_string()
        }
    } else if base.is_empty() {
        "Runtime degraded (legacy fallback active)".to_string()
    } else if let Some(reason) = reason.filter(|text| !text.trim().is_empty()) {
        format!("{base} • Runtime degraded ({})", reason.trim())
    } else {
        format!("{base} • Runtime degraded (legacy fallback active)")
    }
}

pub fn update_voice_chat_context_summary_impl(summary: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.context_text = summary.to_string();
    apply_status_pill(&state);
}

pub fn apply_status_pill(state: &VoiceChatOverlayState) {
    unsafe {
        let Some(pill_ptr) = state.status_pill else {
            return;
        };
        let palette = state.status_kind.palette();
        let pill = pill_ptr as Id;
        let layer: Id = msg_send![pill, layer];
        if !layer.is_null() {
            let bg = ui_colors::panel_bg();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
            let border = ui_colors::header_border();
            let cg_border: Id = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBorderColor: cg_border];
            let _: () = msg_send![layer, setBorderWidth: ui_tokens::SURFACE_BORDER_WIDTH];
        }

        if let Some(label_ptr) = state.status_pill_label {
            let label = label_ptr as Id;
            let _: () = msg_send![label, setStringValue: ns_string(state.status_kind.label())];
            let text_color = ui_colors::bubble_meta_text();
            let _: () = msg_send![label, setTextColor: text_color];
        }

        if let Some(dot_ptr) = state.status_pill_dot {
            let dot = dot_ptr as Id;
            let dot_layer: Id = msg_send![dot, layer];
            if !dot_layer.is_null() {
                let dot_color = color_rgba(palette.dot.0, palette.dot.1, palette.dot.2, 0.92);
                let cg: Id = msg_send![dot_color, CGColor];
                let _: () = msg_send![dot_layer, setBackgroundColor: cg];
                // Pulse animation for Listening state
                let pulse_key = ns_string("pulse");
                if state.status_kind == UiStatus::Listening {
                    let existing: Id = msg_send![dot_layer, animationForKey: pulse_key];
                    if existing.is_null() {
                        let ca_anim = Class::get("CABasicAnimation").unwrap();
                        let anim: Id =
                            msg_send![ca_anim, animationWithKeyPath: ns_string("opacity")];
                        let from_val: Id =
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 0.95f32];
                        let to_val: Id =
                            msg_send![Class::get("NSNumber").unwrap(), numberWithFloat: 0.55f32];
                        let _: () = msg_send![anim, setFromValue: from_val];
                        let _: () = msg_send![anim, setToValue: to_val];
                        let _: () = msg_send![anim, setDuration: 1.0f64];
                        let _: () = msg_send![anim, setAutoreverses: true];
                        let _: () = msg_send![anim, setRepeatCount: f32::INFINITY];
                        let _: () = msg_send![dot_layer, addAnimation: anim forKey: pulse_key];
                    }
                } else {
                    let _: () = msg_send![dot_layer, removeAnimationForKey: pulse_key];
                }
            }
        }

        let detail = if state.context_text.trim().is_empty() {
            format!("Status: {}", state.status_text)
        } else {
            format!(
                "Status: {} • {}",
                state.status_text,
                state.context_text.trim()
            )
        };
        set_tooltip(pill, &detail);
    }
}
