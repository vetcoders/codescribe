use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::sync::Once;

use crate::ui_helpers::ns_string;

use super::{
    TAB_AI_PROMPTS, TAB_AUDIO_INPUT, TAB_DIAGNOSTICS, TAB_MODES_SHORTCUTS, TAB_TRANSCRIPTION,
    handle_bootstrap_window_closed, handle_hotkey_done, handle_show_overlay, handle_test_mic,
    on_assistive_endpoint_changed, on_assistive_key_changed, on_assistive_model_changed,
    on_beep_toggled, on_clear_assistive_key, on_clear_llm_key, on_copy_diagnostics,
    on_delay_changed, on_diagnostics_refresh, on_double_tap_interval_changed,
    on_enter_send_toggled, on_formatting_level_changed, on_formatting_toggled, on_language_changed,
    on_llm_endpoint_changed, on_llm_key_changed, on_llm_model_changed, on_mode_binding_change,
    on_open_quality_report, on_open_system_settings, on_permission_action, on_prompt_load,
    on_prompt_reset, on_prompt_save, on_prompt_type_changed, on_quality_daemon_toggled,
    on_quality_refresh, on_refresh_permissions, on_save_api_settings, on_show_dock_icon_toggled,
    on_show_hotkey_conflicts, on_ultra_quality_toggled, on_volume_changed, switch_tab,
};

pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();
static TOOLBAR_DELEGATE_INIT: Once = Once::new();
static mut TOOLBAR_DELEGATE_CLASS: *const Class = std::ptr::null();

const NSTOOLBAR_FLEXIBLE_SPACE_ITEM_IDENTIFIER: &str = "NSToolbarFlexibleSpaceItem";

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

            // Tab switching
            decl.add_method(
                sel!(onTabSetup:),
                on_tab_setup as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabModesShortcuts:),
                on_tab_modes_shortcuts as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabAiPrompts:),
                on_tab_ai_prompts as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabAudioInput:),
                on_tab_audio_input as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabDiagnostics:),
                on_tab_diagnostics as extern "C" fn(&Object, Sel, Id),
            );
            // Keys tab actions
            decl.add_method(
                sel!(onModeBindingChange:),
                on_mode_binding_change as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowHotkeyConflicts:),
                on_show_hotkey_conflicts as extern "C" fn(&Object, Sel, Id),
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
                sel!(onFormattingLevelChanged:),
                on_formatting_level_changed as extern "C" fn(&Object, Sel, Id),
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
            decl.add_method(
                sel!(onPromptTypeChanged:),
                on_prompt_type_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPromptLoad:),
                on_prompt_load as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPromptSave:),
                on_prompt_save as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPromptReset:),
                on_prompt_reset as extern "C" fn(&Object, Sel, Id),
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
                sel!(onShowDockIconToggled:),
                on_show_dock_icon_toggled as extern "C" fn(&Object, Sel, Id),
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
            decl.add_method(
                sel!(onUltraQualityToggled:),
                on_ultra_quality_toggled as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onQualityRefresh:),
                on_quality_refresh as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onOpenQualityReport:),
                on_open_quality_report as extern "C" fn(&Object, Sel, Id),
            );

            // Permission refresh
            decl.add_method(
                sel!(onRefreshPermissions:),
                on_refresh_permissions as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPermissionAction:),
                on_permission_action as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onOpenSystemSettings:),
                on_open_system_settings as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDiagnosticsRefresh:),
                on_diagnostics_refresh as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyDiagnostics:),
                on_copy_diagnostics as extern "C" fn(&Object, Sel, Id),
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

pub fn toolbar_delegate_class() -> *const Class {
    unsafe {
        TOOLBAR_DELEGATE_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("BootstrapToolbarDelegate", superclass)
                .expect("Failed to declare toolbar delegate class");
            decl.add_method(
                sel!(toolbarAllowedItemIdentifiers:),
                toolbar_allowed_item_identifiers as extern "C" fn(&Object, Sel, Id) -> Id,
            );
            decl.add_method(
                sel!(toolbarDefaultItemIdentifiers:),
                toolbar_default_item_identifiers as extern "C" fn(&Object, Sel, Id) -> Id,
            );
            decl.add_method(
                sel!(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:),
                toolbar_item_for_identifier as extern "C" fn(&Object, Sel, Id, Id, bool) -> Id,
            );
            TOOLBAR_DELEGATE_CLASS = decl.register();
        });

        TOOLBAR_DELEGATE_CLASS
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

extern "C" fn on_tab_setup(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_TRANSCRIPTION);
}

extern "C" fn on_tab_modes_shortcuts(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_MODES_SHORTCUTS);
}

extern "C" fn on_tab_ai_prompts(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_AI_PROMPTS);
}

extern "C" fn on_tab_audio_input(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_AUDIO_INPUT);
}

extern "C" fn on_tab_diagnostics(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_DIAGNOSTICS);
}

extern "C" fn on_window_will_close(_this: &Object, _sel: Sel, _notification: Id) {
    handle_bootstrap_window_closed();
}

extern "C" fn toolbar_allowed_item_identifiers(_this: &Object, _sel: Sel, _toolbar: Id) -> Id {
    unsafe {
        let ns_mutable_array = Class::get("NSMutableArray").unwrap();
        let ids: Id = msg_send![ns_mutable_array, array];
        // AppKit exposes flexible-space as a global identifier constant, not a class selector.
        let flexible_space: Id = ns_string(NSTOOLBAR_FLEXIBLE_SPACE_ITEM_IDENTIFIER);
        let _: () = msg_send![ids, addObject: flexible_space];
        ids
    }
}

extern "C" fn toolbar_default_item_identifiers(_this: &Object, _sel: Sel, _toolbar: Id) -> Id {
    unsafe {
        let ns_mutable_array = Class::get("NSMutableArray").unwrap();
        let ids: Id = msg_send![ns_mutable_array, array];
        let flexible_space: Id = ns_string(NSTOOLBAR_FLEXIBLE_SPACE_ITEM_IDENTIFIER);
        let _: () = msg_send![ids, addObject: flexible_space];
        ids
    }
}

extern "C" fn toolbar_item_for_identifier(
    _this: &Object,
    _sel: Sel,
    _toolbar: Id,
    item_identifier: Id,
    _will_be_inserted: bool,
) -> Id {
    unsafe {
        let ns_toolbar_item = Class::get("NSToolbarItem").unwrap();
        let item: Id = msg_send![ns_toolbar_item, alloc];
        msg_send![item, initWithItemIdentifier: item_identifier]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_selector_registered(class: *const Class, selector: Sel, label: &str) {
        let responds: bool = unsafe { msg_send![class, instancesRespondToSelector: selector] };
        assert!(
            responds,
            "BootstrapOverlayActionHandler missing selector `{label}`"
        );
    }

    #[test]
    fn action_handler_registers_core_settings_selectors() {
        let class = action_handler_class();
        assert!(
            !class.is_null(),
            "BootstrapOverlayActionHandler class should be registered"
        );

        assert_selector_registered(class, sel!(onTabSetup:), "onTabSetup:");
        assert_selector_registered(class, sel!(onTabModesShortcuts:), "onTabModesShortcuts:");
        assert_selector_registered(class, sel!(onTabAiPrompts:), "onTabAiPrompts:");
        assert_selector_registered(class, sel!(onTabAudioInput:), "onTabAudioInput:");
        assert_selector_registered(class, sel!(onTabDiagnostics:), "onTabDiagnostics:");
        assert_selector_registered(class, sel!(onSaveApiSettings:), "onSaveApiSettings:");
        assert_selector_registered(class, sel!(onPromptSave:), "onPromptSave:");
        assert_selector_registered(class, sel!(onQualityRefresh:), "onQualityRefresh:");
        assert_selector_registered(class, sel!(onPermissionAction:), "onPermissionAction:");
    }
}
