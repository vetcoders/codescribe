//! Plain action trampolines and text-input command handling.
//!
//! Send/close/tab switches, message and card actions, drawer search
//! filtering, recording CTAs, shortcut display, NSTextView delegate
//! callbacks (Enter-to-send, paste interception) and search field reset.

use super::*;

// ═══════════════════════════════════════════════════════════
// Action Handlers
// ═══════════════════════════════════════════════════════════

pub extern "C" fn on_send(_this: &Object, _cmd: Sel, _sender: Id) {
    send_draft_message_impl();
}
pub extern "C" fn on_close(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::ui::voice_chat::api::hide_voice_chat_overlay();
}
pub extern "C" fn on_tab_drawer(_this: &Object, _cmd: Sel, _sender: Id) {
    update_active_tab_impl(Tab::Drawer);
    info!("Tab changed to: {:?}", Tab::Drawer);
}

pub extern "C" fn on_tab_agent(_this: &Object, _cmd: Sel, _sender: Id) {
    update_active_tab_impl(Tab::Agent);
    info!("Tab changed to: {:?}", Tab::Agent);
}

pub extern "C" fn on_tab_settings(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::ui::settings::show_settings_window();
    info!("Settings window opened from chat overlay");
}

pub extern "C" fn on_copy_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(msg) = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
    {
        copy_to_clipboard(&msg.text);
        info!("Copied last assistant response to clipboard");
    } else {
        info!("No assistant response to copy");
    }
}

#[cfg(target_os = "macos")]
fn activate_target_app(app_name: &str) {
    // Activate via NSWorkspace — no shell, no injection surface.
    unsafe {
        let ns_workspace = Class::get("NSWorkspace").unwrap();
        let workspace: Id = msg_send![ns_workspace, sharedWorkspace];
        let running: Id = msg_send![workspace, runningApplications];
        let count: usize = msg_send![running, count];
        for i in 0..count {
            let app: Id = msg_send![running, objectAtIndex: i];
            let name: Id = msg_send![app, localizedName];
            if !name.is_null() {
                let name_cstr: *const std::ffi::c_char = msg_send![name, UTF8String];
                if !name_cstr.is_null() {
                    let name_str = std::ffi::CStr::from_ptr(name_cstr).to_string_lossy();
                    if name_str == app_name {
                        let _: bool = msg_send![app, activateWithOptions: 1u64]; // NSApplicationActivateIgnoringOtherApps
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn paste_last_response_text(text: &str) {
    // Best-effort: if activation fails, paste will likely go nowhere useful;
    // clipboard still contains the response.
    if let Err(e) = crate::os::clipboard::paste_text(text) {
        info!("Paste failed: {}", e);
        copy_to_clipboard(text);
    }
}

pub extern "C" fn on_paste_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let text = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
        .map(|m| m.text.clone());
    let target_app = state.last_target_app.clone();
    drop(state);

    let Some(text) = text else {
        info!("No assistant response to paste");
        return;
    };

    #[cfg(target_os = "macos")]
    {
        let paste_delay_ms = if let Some(app_name) = target_app.as_deref() {
            let app_name = app_name.to_string();
            Queue::main().exec_async(move || activate_target_app(&app_name));
            Some(80_u64)
        } else {
            None
        };

        if let Some(delay_ms) = paste_delay_ms {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                Queue::main().exec_async(move || paste_last_response_text(&text));
            });
        } else {
            Queue::main().exec_async(move || paste_last_response_text(&text));
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        copy_to_clipboard(&text);
    }
}

pub extern "C" fn on_copy_message(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(message) = state.messages.get(index) {
        copy_to_clipboard(&message.text);
    }
}

pub extern "C" fn on_toggle_bubble_render(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    toggle_message_render_mode_impl(index);
}

pub extern "C" fn on_assistant_bubble_click(_this: &Object, _cmd: Sel, sender: Id) {
    handle_message_bubble_click_from_recognizer(sender);
}

pub extern "C" fn on_agent_scroll_live(_this: &Object, _cmd: Sel, _notification: Id) {
    handle_agent_scroll_live();
}

pub extern "C" fn on_latest_message(_this: &Object, _cmd: Sel, _sender: Id) {
    pin_agent_scroll_to_latest_impl();
}

pub extern "C" fn on_card_copy(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_copy(index);
}

pub extern "C" fn on_card_restore(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_restore(index);
}

pub extern "C" fn on_card_edit(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_edit(index);
}

pub extern "C" fn on_card_delete(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_delete(index);
}

pub extern "C" fn on_card_favorite(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    handle_card_favorite(index);
}

fn filter_drawer_for_search_field(search_field: Id) {
    if search_field.is_null() {
        return;
    }
    let is_active_search_field = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.search_field == Some(search_field as usize)
    };
    if !is_active_search_field {
        return;
    }
    let query = unsafe { get_text_field_string(search_field) };
    filter_drawer(&query);
}

pub extern "C" fn on_search_changed(_this: &Object, _cmd: Sel, sender: Id) {
    filter_drawer_for_search_field(sender);
}

pub extern "C" fn on_control_text_did_change(_this: &Object, _cmd: Sel, notification: Id) {
    let search_field: Id = unsafe { msg_send![notification, object] };
    filter_drawer_for_search_field(search_field);
}

pub extern "C" fn on_new_thread(_this: &Object, _cmd: Sel, _sender: Id) {
    start_new_thread_impl();
    info!("New thread requested (backend reset + UI clear)");
}

pub extern "C" fn on_toggle_favorites_only(_this: &Object, _cmd: Sel, _sender: Id) {
    toggle_drawer_favorites_only_impl();
    info!("Toggled Drawer favorites-only filter");
}

pub extern "C" fn on_start_recording(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::controller::request_toggle_recording_start(false);
    info!("CTA: start recording");
}

pub extern "C" fn on_header_record(_this: &Object, _cmd: Sel, _sender: Id) {
    // Header record button is chat-native: keep the session in assistive/chat mode.
    crate::ui::overlay::hide_transcription_overlay();
    crate::controller::request_toggle_recording_start(true);
    info!("Header CTA: toggle assistive recording");
}

pub extern "C" fn on_show_overlay(_this: &Object, _cmd: Sel, _sender: Id) {
    if !crate::ui::voice_chat::api::is_voice_chat_overlay_visible() {
        crate::ui::voice_chat::show_voice_chat_overlay();
    }
    crate::ui::voice_chat::show_agent_tab();
    info!("CTA: show/focus overlay");
}

pub extern "C" fn on_commit_message(_this: &Object, _cmd: Sel, _sender: Id) {
    commit_last_user_message_impl();
    info!("Draft message committed");
}

pub extern "C" fn on_commit_pending_followup(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::ui::voice_chat::api::commit_pending_followup_message_impl();
    info!("Pending follow-up sent");
}

pub extern "C" fn on_edit_pending_followup(_this: &Object, _cmd: Sel, _sender: Id) {
    crate::ui::voice_chat::api::edit_pending_followup_message_impl();
    info!("Pending follow-up moved to draft");
}

pub extern "C" fn on_discard_message(_this: &Object, _cmd: Sel, _sender: Id) {
    discard_last_message_impl();
    info!("Draft message discarded");
}
pub extern "C" fn on_show_shortcuts(_this: &Object, _cmd: Sel, _sender: Id) {
    let (hold, toggle) =
        crate::ui::voice_chat::shortcuts_lines(crate::os::hotkeys::ModeHotkeyBindings::load());
    if !crate::ui::voice_chat::api::is_voice_chat_overlay_visible() {
        // This action is wired to overlay/header UI. If it fires while hidden
        // (e.g. stale responder chain), ignore it instead of spawning a ghost window.
        info!("Ignored shortcuts action while overlay hidden");
        return;
    }
    crate::ui::voice_chat::show_agent_tab();
    crate::ui::voice_chat::add_voice_chat_system_message(&format!(
        "Keyboard shortcuts:\n{}\n{}",
        hold, toggle
    ));
    crate::ui::voice_chat::update_voice_chat_status("Shortcuts");
    info!("Displayed keyboard shortcuts inline (non-modal)");
}
pub extern "C" fn on_text_did_change(_this: &Object, _cmd: Sel, _notification: Id) {
    // Runs on main thread. Keep lightweight and only re-layout when height changes.
    crate::ui::voice_chat::api::resize_agent_input_to_draft();
}

/// NSTextView delegate: intercept Enter to send, allow Shift+Enter for newline.
/// Respects `agent_enter_sends` config:
///   true  → Enter sends, Shift+Enter newline (default / Discord-style)
///   false → Enter newline, Cmd+Enter sends   (Mail / Messages-style)
pub extern "C" fn on_do_command_by_selector(
    _this: &Object,
    _cmd: Sel,
    _text_view: Id,
    selector: Sel,
) -> bool {
    // ── Defense-in-depth paste interception ──
    // The Agent input text view overrides paste: for the Edit menu / Cmd+V path.
    // This delegate hook fires only for key-binding initiated paste commands.
    if selector == sel!(paste:) {
        let handled = unsafe { try_paste_as_attachment() };
        if handled {
            return true;
        }
        return false; // default NSTextView paste
    }

    if selector == sel!(moveUp:) {
        return crate::ui::voice_chat::api::recall_previous_prompt();
    }
    if selector == sel!(moveDown:) {
        return crate::ui::voice_chat::api::recall_next_prompt();
    }

    if selector == sel!(insertNewline:) {
        let (shift_held, cmd_held) = unsafe {
            let ns_app = Class::get("NSApplication").unwrap();
            let app: Id = msg_send![ns_app, sharedApplication];
            let event: Id = msg_send![app, currentEvent];
            if event.is_null() {
                (false, false)
            } else {
                let flags: u64 = msg_send![event, modifierFlags];
                // NSEventModifierFlagShift = 1 << 17
                // NSEventModifierFlagCommand = 1 << 20
                ((flags & (1 << 17)) != 0, (flags & (1 << 20)) != 0)
            }
        };
        let config = Config::load();
        let should_send = if config.agent_enter_sends {
            !shift_held // Enter sends, Shift+Enter newline
        } else {
            cmd_held // Cmd+Enter sends, Enter newline
        };
        if should_send {
            send_draft_message_impl();
            return true; // Handled: send message.
        }
        return false; // Let NSTextView insert a newline.
    }
    false // All other commands: default behaviour.
}
pub fn clear_search_field() {
    // Extract pointer under lock, then drop lock BEFORE AppKit calls
    // to avoid deadlock (AppKit callbacks may re-lock OVERLAY_STATE).
    let search_field = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.search_field
    };
    if let Some(sf) = search_field {
        unsafe {
            set_text_field_string(sf as Id, "");
            set_hidden(sf as Id, false);
        }
    }
}
