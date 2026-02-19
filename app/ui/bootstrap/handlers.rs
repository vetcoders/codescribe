use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use std::sync::Once;

use crate::ui_helpers::{
    ns_string, set_button_symbol, set_tooltip, style_toolbar_icon_button, ui_tokens,
};

use super::{
    TAB_AUDIO, TAB_ENGINE, TAB_KEYS, TAB_SETUP, TAB_USER, TAB_VOICE_LAB,
    handle_bootstrap_window_closed, handle_finish, handle_hotkey_done, handle_show_overlay,
    handle_test_mic, on_assistive_endpoint_changed, on_assistive_key_changed,
    on_assistive_model_changed, on_beep_toggled, on_clear_assistive_key, on_clear_llm_key,
    on_delay_changed, on_double_tap_interval_changed, on_enter_send_toggled,
    on_formatting_level_changed, on_formatting_toggled, on_hold_exclusive_changed,
    on_hold_mod_changed, on_language_changed, on_llm_endpoint_changed, on_llm_key_changed,
    on_llm_model_changed, on_open_system_settings, on_permission_action, on_preset_changed,
    on_quality_daemon_toggled, on_refresh_permissions, on_save_api_settings,
    on_show_dock_icon_toggled, on_toggle_trigger_changed, on_ultra_quality_toggled,
    on_voice_lab_field_changed, on_voice_lab_toggle_changed, on_volume_changed, switch_tab,
};

pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();
static TOOLBAR_DELEGATE_INIT: Once = Once::new();
static mut TOOLBAR_DELEGATE_CLASS: *const Class = std::ptr::null();

const SETTINGS_TOOLBAR_ITEM_SHOW_AGENT: &str = "codescribe.toolbar.show-agent";
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
            decl.add_method(
                sel!(onTabVoiceLab:),
                on_tab_voice_lab as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabEngine:),
                on_tab_engine as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabUser:),
                on_tab_user as extern "C" fn(&Object, Sel, Id),
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
                sel!(onFormattingLevelChanged:),
                on_formatting_level_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onVoiceLabToggleChanged:),
                on_voice_lab_toggle_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onVoiceLabFieldChanged:),
                on_voice_lab_field_changed as extern "C" fn(&Object, Sel, Id),
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
            decl.add_method(
                sel!(onToolbarShowOverlay:),
                on_toolbar_show_overlay as extern "C" fn(&Object, Sel, Id),
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

extern "C" fn on_finish(_this: &Object, _sel: Sel, _sender: Id) {
    handle_finish();
}

extern "C" fn on_tab_setup(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_SETUP);
}

extern "C" fn on_tab_keys(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_KEYS);
}

extern "C" fn on_tab_audio(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_AUDIO);
}

extern "C" fn on_tab_voice_lab(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_VOICE_LAB);
}

extern "C" fn on_tab_engine(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_ENGINE);
}

extern "C" fn on_tab_user(_this: &Object, _sel: Sel, _sender: Id) {
    switch_tab(TAB_USER);
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
        let _: () = msg_send![ids, addObject: ns_string(SETTINGS_TOOLBAR_ITEM_SHOW_AGENT)];
        ids
    }
}

extern "C" fn toolbar_default_item_identifiers(_this: &Object, _sel: Sel, _toolbar: Id) -> Id {
    unsafe {
        let ns_mutable_array = Class::get("NSMutableArray").unwrap();
        let ids: Id = msg_send![ns_mutable_array, array];
        let flexible_space: Id = ns_string(NSTOOLBAR_FLEXIBLE_SPACE_ITEM_IDENTIFIER);
        let _: () = msg_send![ids, addObject: flexible_space];
        let _: () = msg_send![ids, addObject: ns_string(SETTINGS_TOOLBAR_ITEM_SHOW_AGENT)];
        ids
    }
}

extern "C" fn toolbar_item_for_identifier(
    this: &Object,
    _sel: Sel,
    _toolbar: Id,
    item_identifier: Id,
    _will_be_inserted: bool,
) -> Id {
    unsafe {
        let flexible_space_identifier = ns_string(NSTOOLBAR_FLEXIBLE_SPACE_ITEM_IDENTIFIER);
        let is_flexible_space: bool =
            msg_send![item_identifier, isEqualToString: flexible_space_identifier];
        if is_flexible_space {
            let ns_toolbar_item = Class::get("NSToolbarItem").unwrap();
            let item: Id = msg_send![ns_toolbar_item, alloc];
            let item: Id = msg_send![item, initWithItemIdentifier: item_identifier];
            return item;
        }

        let show_agent_identifier = ns_string(SETTINGS_TOOLBAR_ITEM_SHOW_AGENT);
        let is_show_agent: bool =
            msg_send![item_identifier, isEqualToString: show_agent_identifier];
        if !is_show_agent {
            let ns_toolbar_item = Class::get("NSToolbarItem").unwrap();
            let item: Id = msg_send![ns_toolbar_item, alloc];
            let item: Id = msg_send![item, initWithItemIdentifier: item_identifier];
            return item;
        }

        let ns_toolbar_item = Class::get("NSToolbarItem").unwrap();
        let item: Id = msg_send![ns_toolbar_item, alloc];
        let item: Id = msg_send![item, initWithItemIdentifier: item_identifier];
        let _: () = msg_send![item, setLabel: ns_string("Show Agent")];
        let _: () = msg_send![item, setPaletteLabel: ns_string("Show Agent")];
        let _: () = msg_send![item, setToolTip: ns_string("Show agent overlay")];

        let ns_button = Class::get("NSButton").unwrap();
        let button: Id = msg_send![ns_button, alloc];
        let button: Id = msg_send![
            button,
            initWithFrame: CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(ui_tokens::HEADER_BUTTON_SIZE, ui_tokens::HEADER_BUTTON_SIZE),
            )
        ];
        let _: () = msg_send![button, setTitle: ns_string("")];
        let target: Id = this as *const Object as *mut Object;
        let _: () = msg_send![button, setTarget: target];
        let _: () = msg_send![button, setAction: sel!(onToolbarShowOverlay:)];
        let _ = set_button_symbol(button, "bubble.left.and.bubble.right");
        style_toolbar_icon_button(button);
        set_tooltip(button, "Show agent overlay");
        let _: () = msg_send![item, setView: button];
        let _: () = msg_send![
            item,
            setMinSize: CGSize::new(ui_tokens::HEADER_BUTTON_SIZE, ui_tokens::HEADER_BUTTON_SIZE)
        ];
        let _: () = msg_send![
            item,
            setMaxSize: CGSize::new(ui_tokens::HEADER_BUTTON_SIZE, ui_tokens::HEADER_BUTTON_SIZE)
        ];
        item
    }
}

extern "C" fn on_toolbar_show_overlay(_this: &Object, _sel: Sel, _sender: Id) {
    handle_show_overlay();
}
