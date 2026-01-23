//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window that:
//! - Shows status during voice chat (Recording, Thinking, etc.)
//! - Displays streaming LLM response text
//! - Auto-hides after completion
//!
//! Module structure:
//! - `state` - Types, state struct, lazy_static globals
//! - `handlers` - Objective-C class registration and action handlers
//! - `api` - Public API functions and internal helpers

// Allow unexpected cfgs from objc crate's msg_send! macro
// Allow unused API methods - they're part of the public interface for future use

mod api;
mod handlers;
mod state;

// Re-export public API
pub use api::{
    add_voice_chat_error_message, add_voice_chat_user_message, append_voice_chat_assistant_delta,
    append_voice_chat_delta, clear_voice_chat_text, clear_voice_draft, finalize_voice_draft,
    get_accumulated_text, get_voice_draft, hide_voice_chat_overlay, is_auto_send_enabled,
    is_voice_chat_overlay_visible, reset_voice_chat_activity, send_voice_chat_draft,
    set_voice_chat_draft_text, set_voice_chat_send_callback, set_voice_chat_sending,
    set_voice_chat_text, update_voice_chat_status,
};
pub use state::{InputSource, VoiceChatOverlayConfig};

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
use tracing::info;

use crate::ui_helpers::{
    add_subview, button_set_action, button_style, create_button, create_checkbox,
    create_scrollable_text_view, create_vertical_stack_view, set_hidden, stack_view_add,
};

use api::{
    populate_drafts_list, update_chat_view_with_state, update_input_field_with_state,
    update_send_button_with_state,
};
use handlers::{action_handler_class, window_delegate_class};
use state::OVERLAY_STATE;

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

/// Show the voice chat overlay window
pub fn show_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

/// Show the voice chat overlay with custom configuration
pub fn show_voice_chat_overlay_with_config(_config: VoiceChatOverlayConfig) {
    // Currently uses default dimensions, config reserved for future use
    Queue::main().exec_async(|| {
        show_voice_chat_overlay_impl();
    });
}

fn show_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

        // Reuse existing window if any
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, orderFrontRegardless];
            info!("Voice chat overlay reused");
            return;
        }

        // Do NOT clear messages/draft here to ensure persistence
        state.is_sending = false; // Reset sending state on fresh open just in case

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_text_field = Class::get("NSTextField").unwrap();

        // Get screen size to position the overlay
        let ns_screen = Class::get("NSScreen").unwrap();
        let main_screen: Id = msg_send![ns_screen, mainScreen];
        let visible_frame: CGRect = msg_send![main_screen, visibleFrame];

        // Load config for position logic
        let config = Config::load();

        // Mission Control dimensions: split view (60% left panel + 40% right sidecar)
        let window_width = 750.0;
        let window_height = 400.0;
        let margin = 16.0;

        // Split panel layout
        let left_panel_width = 450.0; // 60% for chat + manual input

        let (x, y) = match config.overlay_position_mode {
            OverlayPositionMode::SnappedTopRight => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                (
                    right_x - window_width - margin,
                    top_y - window_height - margin,
                )
            }
            OverlayPositionMode::Custom => {
                let right_x = visible_frame.origin.x + visible_frame.size.width;
                let top_y = visible_frame.origin.y + visible_frame.size.height;
                let def_x = right_x - window_width - margin;
                let def_y = top_y - window_height - margin;
                (
                    config.overlay_custom_x.unwrap_or(def_x),
                    config.overlay_custom_y.unwrap_or(def_y),
                )
            }
        };

        let frame = CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: window_width,
                height: window_height,
            },
        };

        // Create window with rounded corners style (Title + Closable + FullSizeContent)
        let window: Id = msg_send![ns_window, alloc];
        let style_mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::FullSizeContentView;
        let backing = NSBackingStoreType::Buffered;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style_mask
            backing: backing
            defer: false
        ];

        // Configure rounded corners and dragging
        let _: () = msg_send![window, setTitleVisibility: 1]; // NSWindowTitleHidden
        let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        // Configure window appearance
        let bg_color = NSColor::colorWithCalibratedRed_green_blue_alpha(0.1, 0.1, 0.1, 0.95);
        let bg_color_ptr = &*bg_color as *const _ as Id;
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setBackgroundColor: bg_color_ptr];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        // Get content view
        let content_view: Id = msg_send![window, contentView];

        // --- LAYOUT ---
        // Top: Header (Status)
        // Below: Input Area
        // Bottom: Chat Log (Reversed flow)

        let header_height = 30.0;
        let input_area_height = 40.0;

        // 1. Status Header (Top)
        let status_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: window_height - header_height,
            },
            size: CGSize {
                width: window_width,
                height: header_height,
            },
        };
        let status_field: Id = msg_send![ns_text_field, alloc];
        let status_field: Id = msg_send![status_field, initWithFrame: status_frame];
        let _: () = msg_send![status_field, setBezeled: false];
        let _: () = msg_send![status_field, setDrawsBackground: true];
        let _: () = msg_send![status_field, setEditable: false];
        let _: () = msg_send![status_field, setSelectable: false];

        let header_color = NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.2, 0.2, 0.8);
        let header_color_ptr = &*header_color as *const _ as Id;
        let _: () = msg_send![status_field, setBackgroundColor: header_color_ptr];

        let white_color = NSColor::whiteColor();
        let white_color_ptr = &*white_color as *const _ as Id;
        let _: () = msg_send![status_field, setTextColor: white_color_ptr];

        let ns_string = Class::get("NSString").unwrap();
        let initial_status: Id = msg_send![ns_string, stringWithUTF8String: c"Ready".as_ptr()];
        let _: () = msg_send![status_field, setStringValue: initial_status];
        let _: () = msg_send![content_view, addSubview: status_field];

        // Collapse button (right side of status header)
        let collapse_frame = CGRect {
            origin: CGPoint {
                x: window_width - 40.0,
                y: window_height - header_height + 3.0,
            },
            size: CGSize {
                width: 30.0,
                height: 24.0,
            },
        };
        let collapse_btn = create_button(collapse_frame, ">|", button_style::ROUNDED);
        add_subview(content_view, collapse_btn);

        // 2. Input Area (Below Header)
        // Input Field + Send Button + Auto-Send Checkbox

        // Checkbox "Auto"
        let checkbox_width = 50.0;
        let send_width = 60.0;
        let input_margin = 8.0;
        let controls_y = window_height - header_height - input_area_height + 5.0;

        // Auto-send Checkbox (Left)
        let checkbox_frame = CGRect {
            origin: CGPoint {
                x: input_margin,
                y: controls_y,
            },
            size: CGSize {
                width: checkbox_width,
                height: 24.0,
            },
        };
        let auto_send_cb = create_checkbox(checkbox_frame, "Auto", state.auto_send_enabled);
        add_subview(content_view, auto_send_cb);

        // Attach button (after Auto checkbox)
        let attach_width = 30.0;
        let attach_frame = CGRect {
            origin: CGPoint {
                x: input_margin + checkbox_width + 4.0,
                y: controls_y,
            },
            size: CGSize {
                width: attach_width,
                height: 24.0,
            },
        };
        let attach_btn = create_button(attach_frame, "📎", button_style::ROUNDED);
        add_subview(content_view, attach_btn);

        // Send Button (Right of left panel)
        let send_frame = CGRect {
            origin: CGPoint {
                x: left_panel_width - send_width - input_margin,
                y: controls_y,
            },
            size: CGSize {
                width: send_width,
                height: 24.0,
            },
        };
        let send_button = create_button(send_frame, "Wyślij", button_style::ROUNDED);
        add_subview(content_view, send_button);

        // Input Field (Middle of left panel, after checkbox and attach button)
        let input_x = input_margin + checkbox_width + 4.0 + attach_width + input_margin;
        let input_width = left_panel_width - input_x - send_width - input_margin * 2.0;
        let input_frame = CGRect {
            origin: CGPoint {
                x: input_x,
                y: controls_y,
            },
            size: CGSize {
                width: input_width,
                height: 24.0,
            },
        };
        let input_field: Id = msg_send![ns_text_field, alloc];
        let input_field: Id = msg_send![input_field, initWithFrame: input_frame];
        let _: () = msg_send![input_field, setEditable: true];
        let _: () = msg_send![input_field, setSelectable: true];
        let _: () = msg_send![input_field, setBezeled: true];
        let _: () = msg_send![input_field, setDrawsBackground: true];
        let placeholder: Id =
            msg_send![ns_string, stringWithUTF8String: c"Napisz wiadomość...".as_ptr()];
        let _: () = msg_send![input_field, setPlaceholderString: placeholder];
        let _: () = msg_send![content_view, addSubview: input_field];

        // Action Handlers
        let handler_class = action_handler_class();
        let handler: Id = msg_send![handler_class, new];

        button_set_action(send_button, handler, sel!(onSend:));
        button_set_action(input_field, handler, sel!(onInputSubmit:));
        button_set_action(auto_send_cb, handler, sel!(onToggleAutoSend:));
        button_set_action(attach_btn, handler, sel!(onAttach:));
        button_set_action(collapse_btn, handler, sel!(onToggleCollapse:));

        // 3. Chat Log (Below Input Area) - constrained to left panel
        let log_y_top = window_height - header_height - input_area_height;
        let scroll_frame = CGRect {
            origin: CGPoint { x: 10.0, y: 10.0 }, // Bottom padding
            size: CGSize {
                width: left_panel_width - 20.0, // Left panel only (60%)
                height: log_y_top - 10.0,       // Remaining height
            },
        };

        // Create scroll view for bubble container
        let ns_scroll_view = Class::get("NSScrollView").unwrap();
        let scroll_view: Id = msg_send![ns_scroll_view, alloc];
        let scroll_view: Id = msg_send![scroll_view, initWithFrame: scroll_frame];
        let _: () = msg_send![scroll_view, setHasVerticalScroller: true];
        let _: () = msg_send![scroll_view, setBorderType: 0]; // NSNoBorder
        let _: () = msg_send![scroll_view, setDrawsBackground: false];

        // Create NSStackView for chat bubbles (instead of NSTextView)
        let content_size: CGSize = msg_send![scroll_view, contentSize];
        let stack_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: content_size,
        };
        let bubble_container = create_vertical_stack_view(stack_frame);

        // Make stack view flipped (newest at top) and document view
        let _: () = msg_send![scroll_view, setDocumentView: bubble_container];
        let _: () = msg_send![content_view, addSubview: scroll_view];

        // Create context menu for scroll view with "Copy Last Response" option
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();

        let context_menu: Id = msg_send![ns_menu, alloc];
        let context_menu: Id = msg_send![context_menu, init];

        // "Kopiuj ostatnia odpowiedz" menu item
        let menu_item: Id = msg_send![ns_menu_item, alloc];
        let item_title: Id =
            msg_send![ns_string, stringWithUTF8String: c"Kopiuj ostatnia odpowiedz".as_ptr()];
        let empty_key: Id = msg_send![ns_string, stringWithUTF8String: c"".as_ptr()];
        let menu_item: Id = msg_send![menu_item, initWithTitle: item_title
                                                        action: sel!(onCopyLastResponse:)
                                                 keyEquivalent: empty_key];
        let _: () = msg_send![menu_item, setTarget: handler];
        let _: () = msg_send![context_menu, addItem: menu_item];

        // Attach menu to scroll view
        let _: () = msg_send![scroll_view, setMenu: context_menu];

        // --- RIGHT PANEL (Sidecar) ---
        // 4. Separator line between left and right panels
        let separator_x = left_panel_width;
        let ns_box = Class::get("NSBox").unwrap();
        let separator: Id = msg_send![ns_box, alloc];
        let separator_frame = CGRect {
            origin: CGPoint {
                x: separator_x,
                y: 10.0,
            },
            size: CGSize {
                width: 1.0,
                height: window_height - header_height - 20.0,
            },
        };
        let separator: Id = msg_send![separator, initWithFrame: separator_frame];
        let _: () = msg_send![separator, setBoxType: 1_isize]; // NSBoxSeparator
        let _: () = msg_send![content_view, addSubview: separator];

        // 5. Tab bar (NSSegmentedControl) for right panel
        let ns_segmented = Class::get("NSSegmentedControl").unwrap();
        let tab_bar: Id = msg_send![ns_segmented, alloc];
        let tab_frame = CGRect {
            origin: CGPoint {
                x: separator_x + 10.0,
                y: window_height - header_height - 35.0,
            },
            size: CGSize {
                width: 280.0,
                height: 24.0,
            },
        };
        let tab_bar: Id = msg_send![tab_bar, initWithFrame: tab_frame];
        let _: () = msg_send![tab_bar, setSegmentCount: 2_isize];
        let drafts_label: Id = msg_send![ns_string, stringWithUTF8String: c"Drafts".as_ptr()];
        let settings_label: Id = msg_send![ns_string, stringWithUTF8String: c"Settings".as_ptr()];
        let _: () = msg_send![tab_bar, setLabel: drafts_label forSegment: 0_isize];
        let _: () = msg_send![tab_bar, setLabel: settings_label forSegment: 1_isize];
        let _: () = msg_send![tab_bar, setSelectedSegment: state.selected_tab as isize];
        let _: () = msg_send![tab_bar, setTarget: handler];
        let _: () = msg_send![tab_bar, setAction: sel!(onTabChanged:)];
        let _: () = msg_send![content_view, addSubview: tab_bar];

        // 6. Drafts list (scroll view with stack view)
        let drafts_buttons_height = 35.0;
        let drafts_list_y = 10.0 + drafts_buttons_height;
        let drafts_list_height = window_height - header_height - 45.0 - drafts_buttons_height;

        let drafts_scroll_frame = CGRect {
            origin: CGPoint {
                x: separator_x + 10.0,
                y: drafts_list_y,
            },
            size: CGSize {
                width: 280.0,
                height: drafts_list_height,
            },
        };

        let drafts_scroll: Id = msg_send![ns_scroll_view, alloc];
        let drafts_scroll: Id = msg_send![drafts_scroll, initWithFrame: drafts_scroll_frame];
        let _: () = msg_send![drafts_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![drafts_scroll, setBorderType: 0]; // NSNoBorder
        let _: () = msg_send![drafts_scroll, setDrawsBackground: false];

        // Stack view for draft items
        let drafts_content_size: CGSize = msg_send![drafts_scroll, contentSize];
        let drafts_stack_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: drafts_content_size,
        };
        let drafts_container = create_vertical_stack_view(drafts_stack_frame);
        let _: () = msg_send![drafts_scroll, setDocumentView: drafts_container];
        let _: () = msg_send![content_view, addSubview: drafts_scroll];

        // Draft editor (hidden until edit action)
        let (draft_editor_scroll, draft_editor_view) =
            create_scrollable_text_view(drafts_scroll_frame, true);
        add_subview(content_view, draft_editor_scroll);
        set_hidden(draft_editor_scroll, true);

        // 7. Edit and Copy buttons at bottom of drafts panel
        let btn_width = 70.0;
        let btn_spacing = 10.0;
        let btn_y = 10.0;

        let edit_btn_frame = CGRect {
            origin: CGPoint {
                x: separator_x + 10.0,
                y: btn_y,
            },
            size: CGSize {
                width: btn_width,
                height: 24.0,
            },
        };
        let edit_btn = create_button(edit_btn_frame, "Edit", button_style::ROUNDED);
        button_set_action(edit_btn, handler, sel!(onDraftEdit:));
        add_subview(content_view, edit_btn);

        let copy_btn_frame = CGRect {
            origin: CGPoint {
                x: separator_x + 10.0 + btn_width + btn_spacing,
                y: btn_y,
            },
            size: CGSize {
                width: btn_width,
                height: 24.0,
            },
        };
        let copy_btn = create_button(copy_btn_frame, "Copy", button_style::ROUNDED);
        button_set_action(copy_btn, handler, sel!(onDraftCopy:));
        add_subview(content_view, copy_btn);

        // 8. Settings panel (hidden by default, shown when tab 1 is selected)
        let settings_scroll_frame = CGRect {
            origin: CGPoint {
                x: separator_x + 10.0,
                y: drafts_list_y,
            },
            size: CGSize {
                width: 280.0,
                height: drafts_list_height,
            },
        };

        let settings_scroll: Id = msg_send![ns_scroll_view, alloc];
        let settings_scroll: Id = msg_send![settings_scroll, initWithFrame: settings_scroll_frame];
        let _: () = msg_send![settings_scroll, setHasVerticalScroller: true];
        let _: () = msg_send![settings_scroll, setBorderType: 0]; // NSNoBorder
        let _: () = msg_send![settings_scroll, setDrawsBackground: false];

        // Stack view for settings items
        let settings_content_size: CGSize = msg_send![settings_scroll, contentSize];
        let settings_stack_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: settings_content_size,
        };
        let settings_container = create_vertical_stack_view(settings_stack_frame);
        let _: () = msg_send![settings_scroll, setDocumentView: settings_container];

        // Add settings items to stack view
        let item_height = 30.0;
        let item_width = 260.0;

        // AI Formatting checkbox
        let core_config = codescribe_core::config::Config::load();
        let ai_enabled = core_config.ai_formatting_enabled;
        let ai_cb_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: item_width,
                height: item_height,
            },
        };
        let ai_checkbox = create_checkbox(ai_cb_frame, "AI Formatting", ai_enabled);
        button_set_action(ai_checkbox, handler, sel!(onSettingsAiFormatting:));
        stack_view_add(settings_container, ai_checkbox);

        // Edit Config button
        let edit_config_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: item_width,
                height: item_height,
            },
        };
        let edit_config_btn =
            create_button(edit_config_frame, "Edit Config File", button_style::ROUNDED);
        button_set_action(edit_config_btn, handler, sel!(onSettingsEditConfig:));
        stack_view_add(settings_container, edit_config_btn);

        // Edit Prompt button
        let edit_prompt_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: item_width,
                height: item_height,
            },
        };
        let edit_prompt_btn =
            create_button(edit_prompt_frame, "Edit AI Prompt", button_style::ROUNDED);
        button_set_action(edit_prompt_btn, handler, sel!(onSettingsEditPrompt:));
        stack_view_add(settings_container, edit_prompt_btn);

        // Open Prompts Folder button
        let open_folder_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: item_width,
                height: item_height,
            },
        };
        let open_folder_btn = create_button(
            open_folder_frame,
            "Open Prompts Folder",
            button_style::ROUNDED,
        );
        button_set_action(open_folder_btn, handler, sel!(onSettingsOpenPromptsFolder:));
        stack_view_add(settings_container, open_folder_btn);

        // Reset Context button
        let reset_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: item_width,
                height: item_height,
            },
        };
        let reset_btn = create_button(reset_frame, "Reset AI Context", button_style::ROUNDED);
        button_set_action(reset_btn, handler, sel!(onSettingsResetContext:));
        stack_view_add(settings_container, reset_btn);

        let _: () = msg_send![content_view, addSubview: settings_scroll];

        // Hide settings panel by default (show drafts)
        set_hidden(settings_scroll, true);

        // Show the window
        let _: () = msg_send![window, orderFrontRegardless];

        state.window = Some(window as usize);
        state.window_delegate = Some(window_delegate as usize);
        state.scroll_view = Some(scroll_view as usize);
        state.bubble_container = Some(bubble_container as usize);
        state.bubble_views.clear(); // Will be populated by update_chat_view_with_state
        state.status_field = Some(status_field as usize);
        state.input_field = Some(input_field as usize);
        state.send_button = Some(send_button as usize);
        state.auto_send_checkbox = Some(auto_send_cb as usize);
        state.attach_button = Some(attach_btn as usize);
        state.action_handler = Some(handler as usize);
        state.tab_bar = Some(tab_bar as usize);
        state.collapse_button = Some(collapse_btn as usize);
        state.drafts_scroll_view = Some(drafts_scroll as usize);
        state.drafts_container = Some(drafts_container as usize);
        state.draft_editor_scroll_view = Some(draft_editor_scroll as usize);
        state.draft_editor_view = Some(draft_editor_view as usize);
        state.draft_edit_button = Some(edit_btn as usize);
        state.draft_copy_button = Some(copy_btn as usize);
        state.settings_scroll_view = Some(settings_scroll as usize);
        state.settings_container = Some(settings_container as usize);
        state.ai_formatting_checkbox = Some(ai_checkbox as usize);

        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&mut state);
        update_send_button_with_state(&mut state);
        populate_drafts_list(&mut state);
        info!("Voice chat overlay shown");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulated_text() {
        // Just verify the function doesn't panic
        let _ = get_accumulated_text();
    }

    #[test]
    fn test_overlay_config_default() {
        let config = VoiceChatOverlayConfig::default();
        assert_eq!(config.width, 750.0); // Mission Control split view
        assert_eq!(config.height, 400.0);
        assert_eq!(config.auto_hide_timeout_secs, 5);
    }

    #[test]
    fn test_overlay_config_custom() {
        let config = VoiceChatOverlayConfig {
            width: 600.0,
            height: 500.0,
            auto_hide_timeout_secs: 10,
        };
        assert_eq!(config.width, 600.0);
        assert_eq!(config.height, 500.0);
        assert_eq!(config.auto_hide_timeout_secs, 10);
    }
}
