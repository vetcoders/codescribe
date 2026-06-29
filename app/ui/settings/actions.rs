//! `extern "C"` action trampolines registered on the settings action handler class.

use super::*;

pub(super) extern "C" fn on_mode_binding_change(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let tag: isize = msg_send![sender, tag];
        if mode_from_double_ctrl_tag(tag) {
            apply_mode_binding(WorkMode::Dictation, ShortcutBinding::DoubleCtrl);
            return;
        }
        if let Some(mode) = mode_from_disable_tag(tag) {
            apply_mode_binding(mode, ShortcutBinding::Disabled);
            return;
        }
        if let Some(mode) = mode_from_tag(tag) {
            start_mode_binding_recorder(mode);
        }
    }
}

pub(super) extern "C" fn on_show_hotkey_conflicts(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    show_hotkey_conflicts_sheet();
}

pub(super) extern "C" fn on_language_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let lang = match idx {
            0 => "auto",
            1 => "pl",
            2 => "en",
            _ => "auto",
        };
        info!("Settings: language -> {}", lang);
        let config = Config::load();
        let _ = config.save_to_env("WHISPER_LANGUAGE", lang);
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_formatting_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: AI formatting -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("AI_FORMATTING_ENABLED", if enabled { "1" } else { "0" });
    }
}

pub(super) extern "C" fn on_transcript_tagging_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: transcript tagging -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "TRANSCRIPT_TAGGING_ENABLED",
            if enabled { "1" } else { "0" },
        );
    }
}

pub(super) extern "C" fn on_formatting_level_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let idx: isize = msg_send![sender, indexOfSelectedItem];
        let level = match idx {
            0 => "raw",
            1 => "medium",
            2 => "creative",
            _ => "medium",
        };
        info!("Settings: Formatting level -> {}", level);
        let config = Config::load();
        let _ = config.save_to_env("FORMATTING_LEVEL", level);
    }
}

pub(super) extern "C" fn on_llm_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: formatting endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_FORMATTING_ENDPOINT", &value);
    }
}

pub(super) extern "C" fn on_llm_model_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: formatting model -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_FORMATTING_MODEL", &value);
    }
}

pub(super) extern "C" fn on_llm_key_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        if !value.is_empty() {
            info!("Settings: formatting API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("LLM_FORMATTING_API_KEY", &value);
            update_keychain_status_labels();
        }
    }
}

pub(super) extern "C" fn on_clear_llm_key(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let field_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.llm_key_field
    };
    clear_keychain_entry("LLM_FORMATTING_API_KEY", field_ptr);
}

pub(super) extern "C" fn on_save_api_settings(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let (llm_endpoint, llm_model, llm_key, assist_endpoint, assist_model, assist_key) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.llm_endpoint_field,
            state.llm_model_field,
            state.llm_key_field,
            state.assistive_endpoint_field,
            state.assistive_model_field,
            state.assistive_key_field,
        )
    };

    let mut entries: Vec<(&str, String)> = Vec::new();
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = llm_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_FORMATTING_ENDPOINT", value.trim().to_string()));
        }
        if let Some(ptr) = llm_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_FORMATTING_MODEL", value.trim().to_string()));
        }
        if let Some(ptr) = llm_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                entries.push(("LLM_FORMATTING_API_KEY", value.trim().to_string()));
            }
        }
        if let Some(ptr) = assist_endpoint {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_ASSISTIVE_ENDPOINT", value.trim().to_string()));
        }
        if let Some(ptr) = assist_model {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            entries.push(("LLM_ASSISTIVE_MODEL", value.trim().to_string()));
        }
        if let Some(ptr) = assist_key {
            let value = crate::ui_helpers::get_text_field_string(ptr as Id);
            if !value.trim().is_empty() {
                entries.push(("LLM_ASSISTIVE_API_KEY", value.trim().to_string()));
            }
        }
    }
    if !entries.is_empty() {
        let config = Config::load();
        let borrowed: Vec<(&str, &str)> = entries.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let _ = config.save_to_env_many(&borrowed);
    }
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        if let Some(ptr) = llm_key {
            set_text_field_string(ptr as Id, "");
        }
        if let Some(ptr) = assist_key {
            set_text_field_string(ptr as Id, "");
        }
    }
    update_keychain_status_labels();
    info!("Settings: API settings saved");
}

pub(super) extern "C" fn on_prompt_type_changed(
    this: &Object,
    cmd: objc::runtime::Sel,
    sender: Id,
) {
    refresh_prompt_editor_labels();
    on_prompt_load(this, cmd, sender);
}

pub(super) extern "C" fn on_prompt_load(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    match load_prompt_content(prompt_type) {
        Ok(content) => {
            set_prompt_editor_content(&content);
            set_prompt_editor_status(
                &format!("{} prompt loaded.", prompt_display_name(prompt_type)),
                false,
            );
        }
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to load prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_prompt_save(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    let content = read_prompt_editor_content();
    if content.trim().is_empty() {
        set_prompt_editor_status("Prompt is empty. Add content before saving.", true);
        return;
    }

    match save_prompt_content(prompt_type, &content) {
        Ok(()) => {
            set_prompt_editor_status(
                &format!("{} prompt saved.", prompt_display_name(prompt_type)),
                false,
            );
        }
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to save prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_prompt_reset(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    let prompt_type = selected_prompt_type();
    match reset_prompt_content(prompt_type) {
        Ok(()) => match load_prompt_content(prompt_type) {
            Ok(content) => {
                set_prompt_editor_content(&content);
                set_prompt_editor_status(
                    &format!(
                        "{} prompt reset to default.",
                        prompt_display_name(prompt_type)
                    ),
                    false,
                );
            }
            Err(err) => {
                set_prompt_editor_status(&format!("Prompt reset but reload failed: {err}"), true);
            }
        },
        Err(err) => {
            set_prompt_editor_status(&format!("Failed to reset prompt: {err}"), true);
        }
    }
    refresh_prompt_editor_labels();
}

pub(super) extern "C" fn on_quality_refresh(_this: &Object, _cmd: objc::runtime::Sel, _sender: Id) {
    refresh_quality_dashboard();
}

pub(super) extern "C" fn on_open_qube_report(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    if !crate::qube_daemon::open_latest_report() {
        warn!("Settings: no quality report available");
    }
    refresh_quality_dashboard();
}

pub(super) extern "C" fn on_diagnostics_refresh(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    refresh_permission_indicators();
    refresh_diagnostics_dashboard();
}

pub(super) extern "C" fn on_copy_diagnostics(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let report = crate::os::permissions::diagnostics_report();
    let (status_ptr, secondary) = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        (
            state.diagnostics_status_label,
            crate::ui_helpers::color_secondary_label(),
        )
    };
    match crate::os::clipboard::set_clipboard(&report) {
        Ok(()) => {
            if let Some(ptr) = status_ptr {
                // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
                unsafe {
                    let label = ptr as Id;
                    set_text_field_string(label, "Diagnostics copied to clipboard.");
                    let _: () = msg_send![label, setTextColor: secondary];
                }
            }
        }
        Err(err) => {
            if let Some(ptr) = status_ptr {
                // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
                unsafe {
                    let label = ptr as Id;
                    set_text_field_string(label, &format!("Failed to copy diagnostics: {err}"));
                    let _: () = msg_send![label, setTextColor: ui_colors::bubble_error_text()];
                }
            }
        }
    }
}

pub(super) extern "C" fn on_delay_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: hold delay -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("HOLD_START_DELAY_MS", &ms.to_string());
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.hold_start_delay_ms = ms;
        hotkeys::apply_hotkey_runtime_config(runtime_config);
        let label_ptr = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.hold_delay_value_label
        };
        if let Some(ptr) = label_ptr {
            set_text_field_string(ptr as Id, &format!("{ms} ms"));
        }
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_double_tap_interval_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: double-tap interval -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("DOUBLE_TAP_INTERVAL_MS", &ms.to_string());
        let mut runtime_config = hotkeys::HotkeyRuntimeConfig::from(&config);
        runtime_config.double_tap_interval_ms = ms;
        hotkeys::apply_hotkey_runtime_config(runtime_config);
        let label_ptr = {
            let state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.double_tap_value_label
        };
        if let Some(ptr) = label_ptr {
            set_text_field_string(ptr as Id, &format!("{ms} ms"));
        }
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_beep_toggled(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: beep on start -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("BEEP_ON_START", if enabled { "1" } else { "0" });
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_enter_send_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: agent enter sends -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("AGENT_ENTER_SENDS", if enabled { "1" } else { "0" });
    }
}

pub(super) extern "C" fn on_show_dock_icon_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: show dock icon -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("SHOW_DOCK_ICON", if enabled { "1" } else { "0" });
        crate::apply_dock_icon_visibility(enabled);
    }
}

pub(super) extern "C" fn on_transcription_overlay_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: transcription overlay -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "TRANSCRIPTION_OVERLAY_ENABLED",
            if enabled { "1" } else { "0" },
        );
        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_preset_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let selected: isize = msg_send![sender, selectedSegment];
        let preset = PreviewTimingPreset::from_segment_index(selected);
        {
            let mut state = SETTINGS_WINDOW_STATE
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.preview_advanced_expanded = matches!(preset, PreviewTimingPreset::Custom);
            state.preview_timing_forced_custom = matches!(preset, PreviewTimingPreset::Custom);
        }

        if matches!(preset, PreviewTimingPreset::Custom) {
            refresh_preview_advanced_visibility();
            refresh_transcription_preview_panel();
            return;
        }

        let config = Config::load();
        if matches!(preset, PreviewTimingPreset::Off) {
            info!("Settings: preview preset -> Off");
            if let Err(err) = config.save_to_env("TRANSCRIPTION_OVERLAY_ENABLED", "0") {
                warn!("Settings: failed to save preview preset Off: {err}");
            }
        } else if let Some(values) = preset_values(preset) {
            info!("Settings: preview preset -> {:?}", preset);
            let entries: Vec<(&str, String)> = vec![
                ("TRANSCRIPTION_OVERLAY_ENABLED", "1".to_string()),
                (
                    "CODESCRIBE_BUFFER_DELAY_MS",
                    values.buffer_delay_ms.to_string(),
                ),
                ("CODESCRIBE_TYPING_CPS", format!("{:.1}", values.typing_cps)),
                (
                    "CODESCRIBE_EMIT_WORDS_MAX",
                    values.emit_words_max.to_string(),
                ),
                (
                    "CODESCRIBE_BUFFERED_INTERIM_SEC",
                    format!("{:.1}", values.interim_sec),
                ),
            ];
            let borrowed: Vec<(&str, &str)> = entries
                .iter()
                .map(|(key, value)| (*key, value.as_str()))
                .collect();
            if let Err(err) = config.save_to_env_many(&borrowed) {
                warn!(
                    "Settings: failed to save preview preset {:?}: {err}",
                    preset
                );
            }
        }

        sync_runtime_config_via_ipc();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_advanced_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    {
        let mut state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.preview_advanced_expanded = !state.preview_advanced_expanded;
    }
    refresh_preview_advanced_visibility();
}

fn mark_preview_timing_custom() {
    let mut state = SETTINGS_WINDOW_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    state.preview_advanced_expanded = true;
    state.preview_timing_forced_custom = true;
}

pub(super) extern "C" fn on_preview_buffer_delay_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let ms = value.round() as u64;
        info!("Settings: preview buffer delay -> {}ms", ms);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_BUFFER_DELAY_MS", &ms.to_string());
        sync_runtime_config_via_ipc();
        mark_preview_timing_custom();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_typing_cps_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let cps = value.max(5.0) as f32;
        info!("Settings: preview typing cps -> {:.1}", cps);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_TYPING_CPS", &format!("{cps:.1}"));
        sync_runtime_config_via_ipc();
        mark_preview_timing_custom();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_emit_words_max_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let words = value.round().clamp(1.0, 10.0) as u64;
        info!("Settings: preview emit words max -> {}", words);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_EMIT_WORDS_MAX", &words.to_string());
        sync_runtime_config_via_ipc();
        mark_preview_timing_custom();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_preview_interim_cadence_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        let secs = value.clamp(1.0, 12.0) as f32;
        info!("Settings: preview interim cadence -> {:.1}s", secs);
        let config = Config::load();
        let _ = config.save_to_env("CODESCRIBE_BUFFERED_INTERIM_SEC", &format!("{secs:.1}"));
        sync_runtime_config_via_ipc();
        mark_preview_timing_custom();
        refresh_transcription_preview_panel();
    }
}

pub(super) extern "C" fn on_stt_provider_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let selected_idx: isize = msg_send![sender, indexOfSelectedItem];
        let use_local_stt = selected_idx == 0;
        info!(
            "Settings: final transcript path -> {}",
            if use_local_stt { "local" } else { "cloud" }
        );
        let config = Config::load();
        let _ = config.save_to_env("USE_LOCAL_STT", if use_local_stt { "1" } else { "0" });
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_stt_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .trim()
            .to_string();
        info!("Settings: STT endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("STT_ENDPOINT", &value);
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_stt_key_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr)
            .to_string_lossy()
            .trim()
            .to_string();
        if value.is_empty() {
            info!("Settings: clearing cloud STT API key from Keychain");
            if let Err(e) = keychain::delete_key("STT_API_KEY") {
                warn!("Failed to delete STT_API_KEY from Keychain: {e}");
            }
            std::env::remove_var("STT_API_KEY");
        } else {
            info!("Settings: cloud STT API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("STT_API_KEY", &value);
        }
        sync_runtime_config_via_ipc();
    }
}

pub(super) extern "C" fn on_volume_changed(_this: &Object, _cmd: objc::runtime::Sel, sender: Id) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let value: f64 = msg_send![sender, doubleValue];
        info!("Settings: sound volume -> {:.2}", value);
        let config = Config::load();
        let _ = config.save_to_env("SOUND_VOLUME", &format!("{:.2}", value));
        sync_runtime_config_via_ipc();
    }
}

// ============================================================================
// Assistive AI + Quality daemon + Permissions handlers
// ============================================================================

pub(super) extern "C" fn on_assistive_endpoint_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: assistive endpoint -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_ASSISTIVE_ENDPOINT", &value);
    }
}

pub(super) extern "C" fn on_assistive_model_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        info!("Settings: assistive model -> {}", value);
        let config = Config::load();
        let _ = config.save_to_env("LLM_ASSISTIVE_MODEL", &value);
    }
}

pub(super) extern "C" fn on_assistive_key_changed(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let ns_val: Id = msg_send![sender, stringValue];
        let cstr: *const std::ffi::c_char = msg_send![ns_val, UTF8String];
        let value = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
        if !value.is_empty() {
            info!("Settings: assistive API key updated (stored in Keychain)");
            let config = Config::load();
            let _ = config.save_to_env("LLM_ASSISTIVE_API_KEY", &value);
            update_keychain_status_labels();
        }
    }
}

pub(super) extern "C" fn on_clear_assistive_key(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    let field_ptr = {
        let state = SETTINGS_WINDOW_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.assistive_key_field
    };
    clear_keychain_entry("LLM_ASSISTIVE_API_KEY", field_ptr);
}

pub(super) extern "C" fn on_qube_daemon_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: quality daemon autostart -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env("QUBE_DAEMON_AUTOSTART", if enabled { "1" } else { "0" });
        if enabled {
            let _ = crate::qube_lifecycle::start_managed();
        } else {
            let _ = crate::qube_lifecycle::stop_managed();
        }
        refresh_quality_dashboard();
        crate::ui::tray::update_quality_label();
    }
}

pub(super) extern "C" fn on_ultra_quality_toggled(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let state: isize = msg_send![sender, state];
        let enabled = state == 1;
        info!("Settings: ultra quality final pass -> {}", enabled);
        let config = Config::load();
        let _ = config.save_to_env(
            "CODESCRIBE_LOCAL_STT_FINAL_PASS",
            if enabled { "1" } else { "0" },
        );
        refresh_quality_dashboard();
    }
}

pub(super) extern "C" fn on_permission_action(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    sender: Id,
) {
    // SAFETY: see module-level # Safety doc — main-thread AppKit / msg_send! access on retained `Id` pointers.
    unsafe {
        let tag: isize = msg_send![sender, tag];
        if let Some(kind) = permission_kind_from_tag(tag) {
            info!("Settings: permission action for {:?}", kind);
            handle_permission_action(kind);
        }
    }
}

pub(super) extern "C" fn on_open_system_settings(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    info!("Settings: opening System Settings");
    open_system_settings_security();
}

pub(super) extern "C" fn on_refresh_permissions(
    _this: &Object,
    _cmd: objc::runtime::Sel,
    _sender: Id,
) {
    info!("Settings: refreshing permission indicators");
    refresh_permission_indicators();
}
