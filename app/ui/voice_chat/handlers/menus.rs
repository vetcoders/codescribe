//! Popup menus anchored to overlay buttons.
//!
//! Attach menu (files / GitHub / URL / clear), export menu with All and
//! Assistant-only submenus, the overflow "more" menu, and the export
//! trampoline actions they target.

use super::*;

pub extern "C" fn on_attach_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        let attach: Id = msg_send![ns_menu_item, alloc];
        let attach: Id = msg_send![
            attach,
            initWithTitle: ns_string("Attach files…")
            action: sel!(onAttachPick:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![attach, setTarget: target];
        let _: () = msg_send![menu, addItem: attach];

        let github: Id = msg_send![ns_menu_item, alloc];
        let github: Id = msg_send![
            github,
            initWithTitle: ns_string("Attach from GitHub…")
            action: sel!(onAttachGitHub:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![github, setTarget: target];
        let _: () = msg_send![menu, addItem: github];

        let url: Id = msg_send![ns_menu_item, alloc];
        let url: Id = msg_send![
            url,
            initWithTitle: ns_string("Attach from URL…")
            action: sel!(onAttachURL:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![url, setTarget: target];
        let _: () = msg_send![menu, addItem: url];

        let count = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.len()
        };
        if count > 0 {
            let sep: Id = msg_send![ns_menu_item, separatorItem];
            let _: () = msg_send![menu, addItem: sep];

            let clear_title = format!("Clear attachments ({})", count);
            let clear: Id = msg_send![ns_menu_item, alloc];
            let clear: Id = msg_send![
                clear,
                initWithTitle: ns_string(&clear_title)
                action: sel!(onAttachClear:)
                keyEquivalent: ns_string("")
            ];
            let _: () = msg_send![clear, setTarget: target];
            let _: () = msg_send![menu, addItem: clear];
        }

        // Pop up anchored at the button.
        let bounds: CGRect = msg_send![sender, bounds];
        let location = CGPoint::new(0.0, bounds.size.height);
        let nil_item: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: nil_item
            atLocation: location
            inView: sender
        ];
    }
}
pub extern "C" fn on_export_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        // Submenu: All
        let all_item: Id = msg_send![ns_menu_item, alloc];
        let all_item: Id = msg_send![
            all_item,
            initWithTitle: ns_string("All")
            action: std::ptr::null_mut::<Object>()
            keyEquivalent: ns_string("")
        ];
        let all_menu: Id = msg_send![ns_menu, new];

        let all_copy: Id = msg_send![ns_menu_item, alloc];
        let all_copy: Id = msg_send![
            all_copy,
            initWithTitle: ns_string("Copy as Markdown")
            action: sel!(onExportAllCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![all_copy, setTarget: target];
        let _: () = msg_send![all_menu, addItem: all_copy];

        let all_save: Id = msg_send![ns_menu_item, alloc];
        let all_save: Id = msg_send![
            all_save,
            initWithTitle: ns_string("Save as Markdown (to history)")
            action: sel!(onExportAllSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![all_save, setTarget: target];
        let _: () = msg_send![all_menu, addItem: all_save];

        let _: () = msg_send![all_item, setSubmenu: all_menu];
        let _: () = msg_send![menu, addItem: all_item];

        // Submenu: Assistant only
        let asst_item: Id = msg_send![ns_menu_item, alloc];
        let asst_item: Id = msg_send![
            asst_item,
            initWithTitle: ns_string("Assistant only")
            action: std::ptr::null_mut::<Object>()
            keyEquivalent: ns_string("")
        ];
        let asst_menu: Id = msg_send![ns_menu, new];

        let asst_copy: Id = msg_send![ns_menu_item, alloc];
        let asst_copy: Id = msg_send![
            asst_copy,
            initWithTitle: ns_string("Copy as Markdown")
            action: sel!(onExportAssistantCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![asst_copy, setTarget: target];
        let _: () = msg_send![asst_menu, addItem: asst_copy];

        let asst_save: Id = msg_send![ns_menu_item, alloc];
        let asst_save: Id = msg_send![
            asst_save,
            initWithTitle: ns_string("Save as Markdown (to history)")
            action: sel!(onExportAssistantSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![asst_save, setTarget: target];
        let _: () = msg_send![asst_menu, addItem: asst_save];

        let _: () = msg_send![asst_item, setSubmenu: asst_menu];
        let _: () = msg_send![menu, addItem: asst_item];

        // Pop up anchored at the button.
        let bounds: CGRect = msg_send![sender, bounds];
        let location = CGPoint::new(0.0, bounds.size.height);
        let nil_item: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: nil_item
            atLocation: location
            inView: sender
        ];
    }
}

pub extern "C" fn on_export_all_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let md = crate::ui::voice_chat::api::export_chat_markdown(false);
    copy_to_clipboard(&md);
    info!("Exported chat (all) to clipboard as Markdown");
}

pub extern "C" fn on_export_all_save(_this: &Object, _cmd: Sel, _sender: Id) {
    if let Some(path) = crate::ui::voice_chat::api::save_chat_markdown_to_history(false) {
        info!("Saved chat (all) export to {}", path.display());
        crate::ui::voice_chat::api::refresh_drawer();
    } else {
        info!("Failed to save chat (all) export");
    }
}

pub extern "C" fn on_export_assistant_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let md = crate::ui::voice_chat::api::export_chat_markdown(true);
    copy_to_clipboard(&md);
    info!("Exported chat (assistant-only) to clipboard as Markdown");
}

pub extern "C" fn on_export_assistant_save(_this: &Object, _cmd: Sel, _sender: Id) {
    if let Some(path) = crate::ui::voice_chat::api::save_chat_markdown_to_history(true) {
        info!("Saved chat (assistant-only) export to {}", path.display());
        crate::ui::voice_chat::api::refresh_drawer();
    } else {
        info!("Failed to save chat (assistant-only) export");
    }
}
pub extern "C" fn on_more_menu(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let menu: Id = msg_send![ns_menu, new];
        let target: Id = (this as *const Object) as Id;

        let new_thread: Id = msg_send![ns_menu_item, alloc];
        let new_thread: Id = msg_send![
            new_thread,
            initWithTitle: ns_string("New thread")
            action: sel!(onNewThread:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![new_thread, setTarget: target];
        let _: () = msg_send![menu, addItem: new_thread];

        let sep: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep];

        let copy_last: Id = msg_send![ns_menu_item, alloc];
        let copy_last: Id = msg_send![
            copy_last,
            initWithTitle: ns_string("Copy last response")
            action: sel!(onCopyLastResponse:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![copy_last, setTarget: target];
        let _: () = msg_send![menu, addItem: copy_last];

        let paste_last: Id = msg_send![ns_menu_item, alloc];
        let paste_last: Id = msg_send![
            paste_last,
            initWithTitle: ns_string("Paste last response")
            action: sel!(onPasteLastResponse:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![paste_last, setTarget: target];
        let _: () = msg_send![menu, addItem: paste_last];

        let sep2: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep2];

        let export_all_copy: Id = msg_send![ns_menu_item, alloc];
        let export_all_copy: Id = msg_send![
            export_all_copy,
            initWithTitle: ns_string("Export all (copy markdown)")
            action: sel!(onExportAllCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![export_all_copy, setTarget: target];
        let _: () = msg_send![menu, addItem: export_all_copy];

        let export_all_save: Id = msg_send![ns_menu_item, alloc];
        let export_all_save: Id = msg_send![
            export_all_save,
            initWithTitle: ns_string("Export all (save markdown)")
            action: sel!(onExportAllSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![export_all_save, setTarget: target];
        let _: () = msg_send![menu, addItem: export_all_save];

        let export_assistant_copy: Id = msg_send![ns_menu_item, alloc];
        let export_assistant_copy: Id = msg_send![
            export_assistant_copy,
            initWithTitle: ns_string("Export assistant (copy markdown)")
            action: sel!(onExportAssistantCopy:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![export_assistant_copy, setTarget: target];
        let _: () = msg_send![menu, addItem: export_assistant_copy];

        let export_assistant_save: Id = msg_send![ns_menu_item, alloc];
        let export_assistant_save: Id = msg_send![
            export_assistant_save,
            initWithTitle: ns_string("Export assistant (save markdown)")
            action: sel!(onExportAssistantSave:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![export_assistant_save, setTarget: target];
        let _: () = msg_send![menu, addItem: export_assistant_save];

        let sep3: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep3];

        let shortcuts_item: Id = msg_send![ns_menu_item, alloc];
        let shortcuts_item: Id = msg_send![
            shortcuts_item,
            initWithTitle: ns_string("Keyboard Shortcuts")
            action: sel!(onShowShortcuts:)
            keyEquivalent: ns_string("?")
        ];
        let _: () = msg_send![shortcuts_item, setTarget: target];
        let _: () = msg_send![shortcuts_item, setKeyEquivalentModifierMask: (1u64 << 20)];
        let _: () = msg_send![menu, addItem: shortcuts_item];

        // Pop up anchored at the button.
        let bounds: CGRect = msg_send![sender, bounds];
        let location = CGPoint::new(0.0, bounds.size.height);
        let nil_item: *mut Object = std::ptr::null_mut();
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: nil_item
            atLocation: location
            inView: sender
        ];
    }
}
