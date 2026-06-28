//! Objective-C bridge for the wizard: action handler and window delegate
//! class registration plus the `extern "C"` callbacks they dispatch to.

use std::sync::OnceLock;

use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};

use super::Id;
use super::actions::{
    finish_onboarding, handle_back_action, handle_primary_action, handle_skip_action,
};
use super::render::render_current_step;
use super::session::release_onboarding_lock;
use super::state::{
    HotkeyModeChoice, LanguageChoice, ONBOARDING_STATE, OnboardingModeChoice, UiRefs,
};

static ACTION_HANDLER_CLASS: OnceLock<&'static Class> = OnceLock::new();
static WINDOW_DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();

pub(super) fn action_handler_class() -> &'static Class {
    ACTION_HANDLER_CLASS.get_or_init(|| unsafe {
        let superclass = Class::get("NSObject").expect("NSObject class missing");
        let mut decl = ClassDecl::new("CodescribeOnboardingActionHandler", superclass)
            .expect("Failed to create onboarding action handler class");

        decl.add_method(
            sel!(onPrimaryAction:),
            on_primary_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onBackAction:),
            on_back_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onSkipAction:),
            on_skip_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onModeSelected:),
            on_mode_selected as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onLanguageSelected:),
            on_language_selected as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onHotkeySelected:),
            on_hotkey_selected as extern "C" fn(&Object, Sel, Id),
        );

        decl.register()
    })
}

pub(super) fn window_delegate_class() -> &'static Class {
    WINDOW_DELEGATE_CLASS.get_or_init(|| unsafe {
        let superclass = Class::get("NSObject").expect("NSObject class missing");
        let mut decl = ClassDecl::new("CodescribeOnboardingWindowDelegate", superclass)
            .expect("Failed to create onboarding window delegate class");
        decl.add_method(
            sel!(windowShouldClose:),
            on_window_should_close as extern "C" fn(&Object, Sel, Id) -> bool,
        );
        decl.add_method(
            sel!(windowWillClose:),
            on_window_will_close as extern "C" fn(&Object, Sel, Id),
        );
        decl.register()
    })
}

extern "C" fn on_primary_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_primary_action();
}

extern "C" fn on_back_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_back_action();
}

extern "C" fn on_skip_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_skip_action();
}

extern "C" fn on_mode_selected(_this: &Object, _sel: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.onboarding_mode = if tag == 1 {
            OnboardingModeChoice::Agentic
        } else {
            OnboardingModeChoice::Basic
        };
    }
    render_current_step();
}

extern "C" fn on_language_selected(_this: &Object, _sel: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.language = match tag {
            1 => LanguageChoice::English,
            2 => LanguageChoice::Polish,
            _ => LanguageChoice::Auto,
        };
    }
    render_current_step();
}

extern "C" fn on_hotkey_selected(_this: &Object, _sel: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_mode = match tag {
            1 => HotkeyModeChoice::Toggle,
            2 => HotkeyModeChoice::Both,
            _ => HotkeyModeChoice::HoldToTalk,
        };
    }
    render_current_step();
}

extern "C" fn on_window_should_close(_this: &Object, _sel: Sel, _sender: Id) -> bool {
    let closing_via_finish = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.closing_via_finish
    };

    if closing_via_finish {
        return true;
    }

    finish_onboarding(false);
    false
}

extern "C" fn on_window_will_close(_this: &Object, _sel: Sel, notification: Id) {
    let window_ptr = unsafe {
        if notification.is_null() {
            None
        } else {
            let window: Id = msg_send![notification, object];
            if window.is_null() {
                None
            } else {
                Some(window as usize)
            }
        }
    };

    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let delegate_ptr = state.window_delegate.take();
    let handler_ptr = state.action_handler.take();
    state.window = None;
    state.ui = UiRefs::default();
    state.full_disk_polling = false;
    state.scheduled_auto_advance_step = None;
    state.closing_via_finish = false;
    drop(state);

    release_onboarding_lock();

    unsafe {
        if let Some(ptr) = delegate_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handler_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = window_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
    }
}
