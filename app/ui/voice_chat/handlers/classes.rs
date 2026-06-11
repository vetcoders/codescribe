//! Objective-C class registration for the voice chat overlay.
//!
//! Declares and caches the action handler, window delegate, overlay window
//! subclass and attachment drop-target classes, plus the CGRect ABI encodings
//! their selector signatures require.

use super::*;

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();
static WINDOW_DELEGATE_INIT: Once = Once::new();
static mut WINDOW_DELEGATE_CLASS: *const Class = std::ptr::null();
static OVERLAY_WINDOW_INIT: Once = Once::new();
static mut OVERLAY_WINDOW_CLASS: *const Class = std::ptr::null();
static AGENT_INPUT_TEXT_VIEW_INIT: Once = Once::new();
static mut AGENT_INPUT_TEXT_VIEW_CLASS: *const Class = std::ptr::null();
static DROP_TARGET_INIT: Once = Once::new();
static mut DROP_TARGET_CLASS: *const Class = std::ptr::null();
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjcCGPoint {
    x: CGFloat,
    y: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjcCGSize {
    width: CGFloat,
    height: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ObjcCGRect {
    origin: ObjcCGPoint,
    size: ObjcCGSize,
}

#[cfg(target_pointer_width = "64")]
const OBJC_POINT_ENCODING: &str = "{CGPoint=dd}";
#[cfg(target_pointer_width = "32")]
const OBJC_POINT_ENCODING: &str = "{CGPoint=ff}";

#[cfg(target_pointer_width = "64")]
const OBJC_SIZE_ENCODING: &str = "{CGSize=dd}";
#[cfg(target_pointer_width = "32")]
const OBJC_SIZE_ENCODING: &str = "{CGSize=ff}";

#[cfg(target_pointer_width = "64")]
const OBJC_RECT_ENCODING: &str = "{CGRect={CGPoint=dd}{CGSize=dd}}";
#[cfg(target_pointer_width = "32")]
const OBJC_RECT_ENCODING: &str = "{CGRect={CGPoint=ff}{CGSize=ff}}";

unsafe impl objc::Encode for ObjcCGPoint {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str(OBJC_POINT_ENCODING) }
    }
}

unsafe impl objc::Encode for ObjcCGSize {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str(OBJC_SIZE_ENCODING) }
    }
}

unsafe impl objc::Encode for ObjcCGRect {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str(OBJC_RECT_ENCODING) }
    }
}

impl From<CGRect> for ObjcCGRect {
    fn from(value: CGRect) -> Self {
        Self {
            origin: ObjcCGPoint {
                x: value.origin.x,
                y: value.origin.y,
            },
            size: ObjcCGSize {
                width: value.size.width,
                height: value.size.height,
            },
        }
    }
}

impl From<ObjcCGRect> for CGRect {
    fn from(value: ObjcCGRect) -> Self {
        CGRect::new(
            &CGPoint::new(value.origin.x, value.origin.y),
            &CGSize::new(value.size.width, value.size.height),
        )
    }
}
/// Get or create the action handler class for UI controls
pub fn action_handler_class() -> *const Class {
    unsafe {
        ACTION_HANDLER_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayActionHandler", superclass)
                .expect("Failed to declare handler class");
            decl.add_method(sel!(onSend:), on_send as extern "C" fn(&Object, Sel, Id));
            decl.add_method(
                sel!(onInputSubmit:),
                on_send as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachMenu:),
                on_attach_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachPick:),
                on_attach_pick as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachClear:),
                on_attach_clear as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabDrawer:),
                on_tab_drawer as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabAgent:),
                on_tab_agent as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabSettings:),
                on_tab_settings as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(sel!(onClose:), on_close as extern "C" fn(&Object, Sel, Id));
            decl.add_method(
                sel!(onCopyLastResponse:),
                on_copy_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onPasteLastResponse:),
                on_paste_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyMessage:),
                on_copy_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onToggleBubbleRender:),
                on_toggle_bubble_render as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAssistantBubbleClick:),
                on_assistant_bubble_click as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAgentScrollLive:),
                on_agent_scroll_live as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onLatestMessage:),
                on_latest_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardCopy:),
                on_card_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardRestore:),
                on_card_restore as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardEdit:),
                on_card_edit as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardDelete:),
                on_card_delete as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCardFavorite:),
                on_card_favorite as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onSearchChanged:),
                on_search_changed as extern "C" fn(&Object, Sel, Id),
            );
            // NSTextField/NSSearchField delegate callback for per-keystroke filtering.
            decl.add_method(
                sel!(controlTextDidChange:),
                on_control_text_did_change as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onNewThread:),
                on_new_thread as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onToggleFavoritesOnly:),
                on_toggle_favorites_only as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onStartRecording:),
                on_start_recording as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onHeaderRecord:),
                on_header_record as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowOverlay:),
                on_show_overlay as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCommitMessage:),
                on_commit_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDiscardMessage:),
                on_discard_message as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportMenu:),
                on_export_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAllCopy:),
                on_export_all_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAllSave:),
                on_export_all_save as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAssistantCopy:),
                on_export_assistant_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onExportAssistantSave:),
                on_export_assistant_save as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onMoreMenu:),
                on_more_menu as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onShowShortcuts:),
                on_show_shortcuts as extern "C" fn(&Object, Sel, Id),
            );
            // Connector actions (GitHub, URL)
            decl.add_method(
                sel!(onAttachGitHub:),
                on_attach_github as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttachURL:),
                on_attach_url as extern "C" fn(&Object, Sel, Id),
            );
            // Attachment chip actions
            decl.add_method(
                sel!(onChipClick:),
                on_chip_click as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipRemove:),
                on_chip_remove as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipPreview:),
                on_chip_preview as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipReveal:),
                on_chip_reveal as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onChipCopyPath:),
                on_chip_copy_path as extern "C" fn(&Object, Sel, Id),
            );
            // NSTextView delegate (auto-resize input bar as content grows/shrinks).
            decl.add_method(
                sel!(textDidChange:),
                on_text_did_change as extern "C" fn(&Object, Sel, Id),
            );
            // Intercept Enter → send, Shift+Enter → newline.
            decl.add_method(
                sel!(textView:doCommandBySelector:),
                on_do_command_by_selector as extern "C" fn(&Object, Sel, Id, Sel) -> bool,
            );
            let cls = decl.register();
            ACTION_HANDLER_CLASS = cls;
        });
        ACTION_HANDLER_CLASS
    }
}

/// Get or create the window delegate class
pub fn window_delegate_class() -> *const Class {
    unsafe {
        WINDOW_DELEGATE_INIT.call_once(|| {
            let superclass = Class::get("NSObject").expect("NSObject not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayWindowDelegate", superclass)
                .expect("Failed to declare window delegate class");
            decl.add_method(
                sel!(windowWillClose:),
                on_window_will_close as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(windowDidEndLiveResize:),
                on_window_did_end_live_resize as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(windowDidResize:),
                on_window_did_resize as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(windowDidChangeScreen:),
                on_window_did_change_screen as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            WINDOW_DELEGATE_CLASS = cls;
        });
        WINDOW_DELEGATE_CLASS
    }
}

/// Get or create the overlay window subclass.
///
/// We use a borderless floating window for the overlay. On macOS, borderless NSWindow
/// instances are often not keyable by default, which prevents typing into NSTextField
/// controls. This subclass opts into key/main status so the Agent input field works
/// when the user clicks the overlay.
pub fn overlay_window_class() -> *const Class {
    unsafe {
        OVERLAY_WINDOW_INIT.call_once(|| {
            let superclass = Class::get("NSWindow").expect("NSWindow not found");
            let mut decl = ClassDecl::new("VoiceChatOverlayWindow", superclass)
                .expect("Failed to declare overlay window class");
            decl.add_method(
                sel!(canBecomeKeyWindow),
                can_become_key_window as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(
                sel!(canBecomeMainWindow),
                can_become_main_window as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(
                sel!(performKeyEquivalent:),
                perform_key_equivalent as extern "C" fn(&Object, Sel, Id) -> bool,
            );
            decl.add_method(
                sel!(constrainFrameRect:toScreen:),
                constrain_frame_rect_to_screen
                    as extern "C" fn(&Object, Sel, ObjcCGRect, Id) -> ObjcCGRect,
            );
            decl.add_method(
                sel!(draggingEntered:),
                on_dragging_entered as extern "C" fn(&Object, Sel, Id) -> u64,
            );
            decl.add_method(
                sel!(draggingUpdated:),
                // Keep the same operation semantics while cursor moves over the drop target.
                on_dragging_entered as extern "C" fn(&Object, Sel, Id) -> u64,
            );
            decl.add_method(
                sel!(performDragOperation:),
                on_perform_drag_operation as extern "C" fn(&Object, Sel, Id) -> bool,
            );
            let cls = decl.register();
            OVERLAY_WINDOW_CLASS = cls;
        });
        OVERLAY_WINDOW_CLASS
    }
}

/// NSTextView subclass for the Agent input bar.
///
/// The Edit menu dispatches Cmd+V as `paste:` directly through the responder
/// chain, bypassing `textView:doCommandBySelector:`. Override `paste:` here so
/// file URLs and standalone clipboard images reach the attachment interceptor.
pub fn agent_input_text_view_class() -> *const Class {
    unsafe {
        AGENT_INPUT_TEXT_VIEW_INIT.call_once(|| {
            let superclass = Class::get("NSTextView").expect("NSTextView not found");
            let mut decl = ClassDecl::new("VoiceChatAgentInputTextView", superclass)
                .expect("Failed to declare agent input text view class");
            decl.add_method(
                sel!(paste:),
                on_agent_input_paste as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            AGENT_INPUT_TEXT_VIEW_CLASS = cls;
        });
        AGENT_INPUT_TEXT_VIEW_CLASS
    }
}

pub extern "C" fn on_agent_input_paste(this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        if try_paste_as_attachment() {
            return;
        }

        let superclass = Class::get("NSTextView").expect("NSTextView not found");
        let _: () = msg_send![super(this, superclass), paste: sender];
    }
}

/// Drop target view for attachments (supports dragging files into the Agent input bar).
pub fn drop_target_view_class() -> *const Class {
    unsafe {
        DROP_TARGET_INIT.call_once(|| {
            let superclass = Class::get("NSView").expect("NSView not found");
            let mut decl = ClassDecl::new("VoiceChatAttachmentDropView", superclass)
                .expect("Failed to declare drop target class");
            decl.add_method(
                sel!(draggingEntered:),
                on_dragging_entered as extern "C" fn(&Object, Sel, Id) -> u64,
            );
            decl.add_method(
                sel!(performDragOperation:),
                on_perform_drag_operation as extern "C" fn(&Object, Sel, Id) -> bool,
            );
            let cls = decl.register();
            DROP_TARGET_CLASS = cls;
        });
        DROP_TARGET_CLASS
    }
}
