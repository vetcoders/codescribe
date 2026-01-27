use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{sel, sel_impl};
use std::sync::Once;

use super::{handle_finish, handle_hotkey_done, handle_show_overlay, handle_test_mic};

pub type Id = *mut Object;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();

pub fn action_handler_class() -> *const Class {
    unsafe {
        ACTION_HANDLER_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("BootstrapOverlayActionHandler", superclass)
                .expect("Failed to declare handler class");
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
                on_hotkey_done as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onFinish:),
                on_finish as extern "C" fn(&Object, Sel, Id),
            );
            ACTION_HANDLER_CLASS = decl.register();
        });

        ACTION_HANDLER_CLASS
    }
}

extern "C" fn on_test_mic(_this: &Object, _sel: Sel, _sender: Id) {
    handle_test_mic();
}

extern "C" fn on_show_overlay(_this: &Object, _sel: Sel, _sender: Id) {
    handle_show_overlay();
}

extern "C" fn on_hotkey_done(_this: &Object, _sel: Sel, _sender: Id) {
    handle_hotkey_done();
}

extern "C" fn on_finish(_this: &Object, _sel: Sel, _sender: Id) {
    handle_finish();
}
