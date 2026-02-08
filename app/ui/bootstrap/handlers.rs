use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{sel, sel_impl};
use std::sync::Once;

use super::{
    handle_bootstrap_window_closed, handle_finish, handle_hotkey_done, handle_show_overlay,
    handle_test_mic, on_assistive_endpoint_changed, on_assistive_key_changed,
    on_assistive_model_changed, on_beep_toggled, on_buffered_toggled, on_clear_assistive_key,
    on_clear_llm_key, on_delay_changed, on_double_tap_interval_changed, on_enter_send_toggled,
    on_formatting_toggled, on_hold_exclusive_changed, on_hold_mod_changed, on_language_changed,
    on_llm_endpoint_changed, on_llm_key_changed, on_llm_model_changed, on_preset_changed,
    on_quality_daemon_toggled, on_refresh_permissions, on_save_api_settings,
    on_toggle_trigger_changed, on_vad_preset_changed, on_volume_changed, switch_tab,
};

pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();

pub fn action_handler_class() -> *const Class {
    unsafe {
        ACTION_HANDLER_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("BootstrapOverlayActionHandler", superclass)
                .expect("Failed to declare handler class");

            // Setup tab actions
            decl.add_method(
                sel!(onTestMic:),
                on_test_mic as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowOverlay:),
                on_show_overlay as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onHotkeyDone:),
                on_hotkey_done_action as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onFinish:),
                on_finish as extern "C" fn(&Object, Sel, Id),
            );

            // Tab switching
            decl.add_method(
                sel!(onTabSetup:),
                on_tab_setup as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabKeys:),
                on_tab_keys as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabAudio:),
                on_tab_audio as extern "C" fn(&Object, Sel, Id),
            );

            // Keys tab actions
            decl.add_method(
                sel!(onHoldModChanged:),
                on_hold_mod_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onHoldExclusiveChanged:),
                on_hold_exclusive_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPresetChanged:),
                on_preset_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onToggleTriggerChanged:),
                on_toggle_trigger_changed as extern "C" fn(&Object, Sel, Id),
            );

            // Audio tab actions
            decl.add_method(
                sel!(onLanguageChanged:),
                on_language_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onFormattingToggled:),
                on_formatting_toggled as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onVadPresetChanged:),
                on_vad_preset_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onBufferedToggled:),
                on_buffered_toggled as extern "C" fn(&Object, Sel, Id),
            );

            // Setup tab: LLM configuration
            decl.add_method(
                sel!(onLlmEndpointChanged:),
                on_llm_endpoint_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onLlmModelChanged:),
                on_llm_model_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onLlmKeyChanged:),
                on_llm_key_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onClearLlmKey:),
                on_clear_llm_key as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSaveApiSettings:),
                on_save_api_settings as extern "C" fn(&Object, Sel, Id),
            );

            // Keys tab: delay slider
            decl.add_method(
                sel!(onDelayChanged:),
                on_delay_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDoubleTapIntervalChanged:),
                on_double_tap_interval_changed as extern "C" fn(&Object, Sel, Id),
            );

            // Audio tab: beep + volume
            decl.add_method(
                sel!(onBeepToggled:),
                on_beep_toggled as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onEnterSendToggled:),
                on_enter_send_toggled as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onVolumeChanged:),
                on_volume_changed as extern "C" fn(&Object, Sel, Id),
            );

            // Assistive AI fields
            decl.add_method(
                sel!(onAssistiveEndpointChanged:),
                on_assistive_endpoint_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAssistiveModelChanged:),
                on_assistive_model_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAssistiveKeyChanged:),
                on_assistive_key_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onClearAssistiveKey:),
                on_clear_assistive_key as extern "C" fn(&Object, Sel, Id),
            );

            // Quality daemon toggle
            decl.add_method(
                sel!(onQualityDaemonToggled:),
                on_quality_daemon_toggled as extern "C" fn(&Object, Sel, Id),
            );

            // Permission refresh
            decl.add_method(
                sel!(onRefreshPermissions:),
                on_refresh_permissions as extern "C" fn(&Object, Sel, Id),
            );

            ACTION_HANDLER_CLASS = decl.register();
        });

        ACTION_HANDLER_CLASS
    }
}

pub fn window_delegate_class() -> *const Class {
    unsafe {
        WINDOW_DELEGATE_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("BootstrapWindowDelegate", superclass)
                .expect("Failed to declare window delegate class");
            decl.add_method(
                sel!(windowWillClose:),
                on_window_will_close as extern "C" fn(&Object, Sel, Id),
            );
            WINDOW_DELEGATE_CLASS = decl.register();
        });

        WINDOW_DELEGATE_CLASS
    }
}

extern "C" fn on_test_mic(_this: &Object, _sel: Sel, _sender: Id) {
    handle_test_mic();
}

extern "C" fn on_show_overlay(_this: &Object, _sel: Sel, _sender: Id) {
    handle_show_overlay();
}

extern "C" fn on_hotkey_done_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_hotkey_done();
}

extern "C" fn on_finish(_this: &Object, _sel: Sel, _sender: Id) {
    handle_finish();
}

extern "C" fn on_tab_setup(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(0);
}

extern "C" fn on_tab_keys(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(1);
}

extern "C" fn on_tab_audio(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(2);
}

extern "C" fn on_window_will_close(_this: &Object, _sel: Sel, _notification: Id) {
    handle_bootstrap_window_closed();
}
