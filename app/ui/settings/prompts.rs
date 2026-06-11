//! AI prompt editor: load/save/reset prompt content and editor layout.

use super::*;

pub(super) fn prompt_type_from_index(index: isize) -> &'static str {
    if index == 1 {
        "assistive"
    } else {
        "formatting"
    }
}

pub(super) fn prompt_display_name(prompt_type: &str) -> &'static str {
    if prompt_type == "assistive" {
        "Assistive"
    } else {
        "Formatting"
    }
}

pub(super) fn selected_prompt_type() -> &'static str {
    let popup_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_type_popup
    };
    let Some(popup_ptr) = popup_ptr else {
        return "formatting";
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let popup = popup_ptr as Id;
        let idx: isize = msg_send![popup, indexOfSelectedItem];
        prompt_type_from_index(idx)
    }
}

pub(super) fn prompt_path_text(prompt_type: &str) -> String {
    if prompt_type == "assistive" {
        crate::get_assistive_prompt_path().display().to_string()
    } else {
        crate::get_formatting_prompt_path().display().to_string()
    }
}

pub(super) fn load_prompt_content(prompt_type: &str) -> Result<String, String> {
    match send_ipc(IpcCommand::GetPrompt {
        prompt_type: prompt_type.to_string(),
    }) {
        Ok(IpcResponse::Prompt(content)) => Ok(content),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            Ok(if prompt_type == "assistive" {
                crate::config::get_assistive_prompt()
            } else {
                crate::config::get_formatting_prompt()
            })
        }
    }
}

pub(super) fn save_prompt_content(prompt_type: &str, content: &str) -> Result<(), String> {
    match send_ipc(IpcCommand::SavePrompt {
        prompt_type: prompt_type.to_string(),
        content: content.to_string(),
    }) {
        Ok(IpcResponse::Ok) => Ok(()),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            let path = if prompt_type == "assistive" {
                crate::config::get_assistive_prompt_path()
            } else {
                crate::config::get_formatting_prompt_path()
            };
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return Err(e.to_string());
            }
            fs::write(path, content).map_err(|e| e.to_string())
        }
    }
}

pub(super) fn reset_prompt_content(prompt_type: &str) -> Result<(), String> {
    match send_ipc(IpcCommand::ResetPrompt {
        prompt_type: prompt_type.to_string(),
    }) {
        Ok(IpcResponse::Ok) => Ok(()),
        Ok(IpcResponse::Error(err)) => Err(err),
        Ok(other) => Err(format!("Unexpected IPC response: {other:?}")),
        Err(err) => {
            warn!("Settings: prompt IPC unavailable, using config fallback: {err}");
            let path = if prompt_type == "assistive" {
                crate::config::get_assistive_prompt_path()
            } else {
                crate::config::get_formatting_prompt_path()
            };
            let default = if prompt_type == "assistive" {
                crate::config::DEFAULT_ASSISTIVE_PROMPT
            } else {
                crate::config::DEFAULT_FORMATTING_PROMPT
            };
            if let Some(parent) = path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                return Err(e.to_string());
            }
            fs::write(path, default).map_err(|e| e.to_string())
        }
    }
}

pub(super) fn set_prompt_editor_content(text: &str) {
    let text_view_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_editor_text_view
    };
    let Some(text_view_ptr) = text_view_ptr else {
        return;
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        set_text_view_string(text_view_ptr as Id, text);
    }
}

pub(super) fn read_prompt_editor_content() -> String {
    let text_view_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_editor_text_view
    };
    let Some(text_view_ptr) = text_view_ptr else {
        return String::new();
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe { get_text_view_string(text_view_ptr as Id) }
}

pub(super) fn set_prompt_editor_status(text: &str, is_error: bool) {
    let status_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.prompt_status_label
    };
    let Some(status_ptr) = status_ptr else {
        return;
    };
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let label = status_ptr as Id;
        set_text_field_string(label, text);
        let color = if is_error {
            ui_colors::bubble_error_text()
        } else {
            crate::ui_helpers::color_secondary_label()
        };
        let _: () = msg_send![label, setTextColor: color];
    }
}

pub(super) fn refresh_prompt_editor_labels() {
    Queue::main().exec_async(move || unsafe {
        let (path_ptr, status_ptr) = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (state.prompt_path_label, state.prompt_status_label)
        };
        let prompt_type = selected_prompt_type();
        if let Some(ptr) = path_ptr {
            let path_text = format!("Path: {}", prompt_path_text(prompt_type));
            set_text_field_string(ptr as Id, &path_text);
        }
        if let Some(ptr) = status_ptr {
            let hint = if prompt_type == "assistive" {
                "Editing assistive prompt."
            } else {
                "Editing formatting prompt."
            };
            set_text_field_string(ptr as Id, hint);
            let _: () =
                msg_send![ptr as Id, setTextColor: crate::ui_helpers::color_secondary_label()];
        }
    });
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PromptEditorLayout {
    pub(super) editor_height: f64,
    pub(super) editor_y: f64,
    pub(super) status_y: f64,
}

pub(super) fn compute_prompt_editor_layout(y: f64, gap: f64) -> PromptEditorLayout {
    // Keep status text and bottom breathing room below the editor so the editor
    // never climbs into API/model/key controls on smaller vertical space.
    let reserved_below_editor = PROMPT_EDITOR_STATUS_HEIGHT + gap + PROMPT_EDITOR_BOTTOM_PADDING;
    let available_editor_height = (y - reserved_below_editor).max(0.0);
    let editor_height = available_editor_height.min(PROMPT_EDITOR_DESIRED_HEIGHT);
    let editor_y = (y - editor_height).max(0.0);
    let status_y = (editor_y - gap).max(0.0);

    PromptEditorLayout {
        editor_height,
        editor_y,
        status_y,
    }
}
