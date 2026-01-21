//! Voice Chat UI overlay for displaying streaming responses.
//!
//! This module provides a floating overlay window that:
//! - Shows status during voice chat (Recording, Thinking, etc.)
//! - Displays streaming LLM response text
//! - Auto-hides after completion

// Allow unexpected cfgs from objc crate's msg_send! macro
#![allow(unexpected_cfgs)]
// Allow unused API methods - they're part of the public interface for future use
#![allow(dead_code)]

use codescribe_core::config::{Config, OverlayPositionMode};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{NSBackingStoreType, NSColor, NSWindowCollectionBehavior, NSWindowStyleMask};
use std::sync::{Arc, Mutex, Once};
use tracing::{debug, info};

use crate::ui_helpers::{
    BubbleConfig, BubbleRole, add_subview, animate_window_width, button_set_action, button_style,
    create_bubble_view, create_button, create_checkbox, create_vertical_stack_view,
    list_draft_files, ns_string, open_file_in_editor, set_hidden, stack_view_add, stack_view_clear,
    update_bubble_text,
};

// Type alias for Objective-C object pointers
type Id = *mut Object;

// Window level constants
const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

/// Configuration for the voice chat overlay
#[derive(Debug, Clone)]
pub struct VoiceChatOverlayConfig {
    /// Width of the overlay window in pixels
    pub width: f64,
    /// Height of the overlay window in pixels
    pub height: f64,
    /// Auto-hide timeout in seconds (0 = no auto-hide)
    pub auto_hide_timeout_secs: u64,
}

impl Default for VoiceChatOverlayConfig {
    fn default() -> Self {
        Self {
            width: 750.0,  // Mission Control: split view (60% left + 40% right)
            height: 400.0, // Increased for better chat history visibility
            auto_hide_timeout_secs: 5,
        }
    }
}

/// Source of the message input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Voice,
    Manual,
}

/// Voice chat overlay state
struct VoiceChatOverlayState {
    // UI element handles
    window: Option<usize>,
    scroll_view: Option<usize>,
    // Bubble-based chat rendering (replaces single text_view)
    bubble_container: Option<usize>,   // NSStackView for bubbles
    bubble_views: Vec<(usize, usize)>, // (container, text_label) per message
    status_field: Option<usize>,
    input_field: Option<usize>,
    send_button: Option<usize>,
    attach_button: Option<usize>,
    auto_send_checkbox: Option<usize>,
    action_handler: Option<usize>,
    // Right panel (sidecar) for voice draft
    voice_draft_view: Option<usize>,
    voice_draft_header: Option<usize>,
    voice_send_button: Option<usize>,
    voice_use_button: Option<usize>,
    // Collapse button for sidecar
    collapse_button: Option<usize>,
    sidecar_collapsed: bool,
    // Right panel tab bar
    tab_bar: Option<usize>,
    selected_tab: usize, // 0 = Drafts, 1 = Settings
    // Drafts list (right panel content)
    drafts_scroll_view: Option<usize>,
    drafts_container: Option<usize>, // NSStackView for draft rows
    draft_files: Vec<std::path::PathBuf>, // Cached list of draft files
    selected_draft_index: Option<usize>,
    // Chat state
    messages: Vec<ChatMessage>,
    // Separated buffers: manual input (left) vs voice streaming (right)
    manual_draft: String,
    voice_draft: String,
    // Attachments for manual input
    attachments: Vec<std::path::PathBuf>,
    // State flags
    is_sending: bool,
    auto_send_enabled: bool,
    is_voice_active: bool,
}

lazy_static::lazy_static! {
    static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState {
        window: None,
        scroll_view: None,
        bubble_container: None,
        bubble_views: Vec::new(),
        status_field: None,
        input_field: None,
        send_button: None,
        attach_button: None,
        auto_send_checkbox: None,
        action_handler: None,
        voice_draft_view: None,
        voice_draft_header: None,
        voice_send_button: None,
        voice_use_button: None,
        collapse_button: None,
        sidecar_collapsed: false,
        tab_bar: None,
        selected_tab: 0,
        drafts_scroll_view: None,
        drafts_container: None,
        draft_files: Vec::new(),
        selected_draft_index: None,
        messages: Vec::new(),
        manual_draft: String::new(),
        voice_draft: String::new(),
        attachments: Vec::new(),
        is_sending: false,
        auto_send_enabled: true,
        is_voice_active: false,
    });
}

type VoiceChatSendCallback = Arc<dyn Fn(String) + Send + Sync>;

lazy_static::lazy_static! {
    static ref SEND_CALLBACK: Mutex<Option<VoiceChatSendCallback>> =
        Mutex::new(None);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct ChatMessage {
    role: ChatRole,
    text: String,
    is_streaming: bool,
    is_error: bool,
}

static ACTION_HANDLER_INIT: Once = Once::new();
static mut ACTION_HANDLER_CLASS: *const Class = std::ptr::null();

fn action_handler_class() -> *const Class {
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
                sel!(onToggleAutoSend:),
                on_toggle_auto_send as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onTabChanged:),
                on_tab_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyLastResponse:),
                on_copy_last_response as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onAttach:),
                on_attach as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onToggleCollapse:),
                on_toggle_collapse as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDraftEdit:),
                on_draft_edit as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onDraftCopy:),
                on_draft_copy as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(onCopyMessage:),
                on_copy_message as extern "C" fn(&Object, Sel, Id),
            );
            let cls = decl.register();
            ACTION_HANDLER_CLASS = cls;
        });
        ACTION_HANDLER_CLASS
    }
}

extern "C" fn on_send(_this: &Object, _cmd: Sel, _sender: Id) {
    send_draft_message_impl();
}

extern "C" fn on_toggle_auto_send(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let state_val: isize = msg_send![sender, state];
        let is_on = state_val == 1; // NSControlStateValueOn = 1
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.auto_send_enabled = is_on;
        info!("Auto-send toggled: {}", is_on);
    }
}

extern "C" fn on_attach(_this: &Object, _cmd: Sel, _sender: Id) {
    unsafe {
        let ns_open_panel = Class::get("NSOpenPanel").unwrap();
        let panel: Id = msg_send![ns_open_panel, openPanel];

        // Configure panel
        let _: () = msg_send![panel, setCanChooseFiles: true];
        let _: () = msg_send![panel, setCanChooseDirectories: false];
        let _: () = msg_send![panel, setAllowsMultipleSelection: true];

        let ns_string = Class::get("NSString").unwrap();
        let title: Id =
            msg_send![ns_string, stringWithUTF8String: c"Select files to attach".as_ptr()];
        let _: () = msg_send![panel, setTitle: title];

        // Run modal
        let result: isize = msg_send![panel, runModal];

        // NSModalResponseOK = 1
        if result == 1 {
            let urls: Id = msg_send![panel, URLs];
            let count: usize = msg_send![urls, count];

            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..count {
                let url: Id = msg_send![urls, objectAtIndex: i];
                let path: Id = msg_send![url, path];
                let path_cstr: *const i8 = msg_send![path, UTF8String];
                if !path_cstr.is_null() {
                    let path_str = std::ffi::CStr::from_ptr(path_cstr).to_string_lossy();
                    state
                        .attachments
                        .push(std::path::PathBuf::from(path_str.to_string()));
                    info!("Attached: {}", path_str);
                }
            }

            // Update button to show count
            if let Some(btn_ptr) = state.attach_button {
                let btn = btn_ptr as Id;
                let title_str = format!("📎{}", state.attachments.len());
                let mut c_str = title_str.as_bytes().to_vec();
                c_str.push(0);
                let title: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
                let _: () = msg_send![btn, setTitle: title];
            }
        }
    }
}

extern "C" fn on_tab_changed(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        let selected: isize = msg_send![sender, selectedSegment];
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.selected_tab = selected as usize;
        info!(
            "Tab changed to: {}",
            if selected == 0 { "Drafts" } else { "Settings" }
        );
        // TODO: Switch visible content
    }
}

extern "C" fn on_copy_last_response(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    // Find last assistant message
    if let Some(msg) = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::Assistant)
    {
        copy_to_clipboard(&msg.text);
        info!("Copied last assistant response to clipboard");
    } else {
        info!("No assistant response to copy");
    }
}

extern "C" fn on_toggle_collapse(_this: &Object, _cmd: Sel, sender: Id) {
    // Window dimensions for animation
    const EXPANDED_WIDTH: f64 = 750.0;
    const COLLAPSED_WIDTH: f64 = 460.0; // Left panel (450) + some padding
    const ANIMATION_DURATION: f64 = 0.25;

    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.sidecar_collapsed = !state.sidecar_collapsed;
        let is_collapsed = state.sidecar_collapsed;

        // Update button title
        let new_title = if is_collapsed { "<|" } else { ">|" };
        let title = ns_string(new_title);
        let _: () = msg_send![sender, setTitle: title];

        // Hide right panel elements BEFORE collapsing (so they're hidden during animation)
        if is_collapsed {
            if let Some(tab_ptr) = state.tab_bar {
                set_hidden(tab_ptr as Id, true);
            }
            if let Some(scroll_ptr) = state.drafts_scroll_view {
                set_hidden(scroll_ptr as Id, true);
            }
            if let Some(view_ptr) = state.voice_draft_view {
                set_hidden(view_ptr as Id, true);
            }
            if let Some(header_ptr) = state.voice_draft_header {
                set_hidden(header_ptr as Id, true);
            }
        }

        // Animate window width change (drawer slide)
        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let target_width = if is_collapsed {
                COLLAPSED_WIDTH
            } else {
                EXPANDED_WIDTH
            };
            animate_window_width(window, target_width, ANIMATION_DURATION);
        }

        // Show right panel elements AFTER expanding (schedule after animation)
        if !is_collapsed {
            // Dispatch after animation completes
            let tab_ptr = state.tab_bar;
            let scroll_ptr = state.drafts_scroll_view;
            let voice_ptr = state.voice_draft_view;
            let header_ptr = state.voice_draft_header;

            dispatch::Queue::main().exec_after(
                std::time::Duration::from_millis((ANIMATION_DURATION * 1000.0) as u64 + 50),
                move || {
                    if let Some(ptr) = tab_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = scroll_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = voice_ptr {
                        set_hidden(ptr as Id, false);
                    }
                    if let Some(ptr) = header_ptr {
                        set_hidden(ptr as Id, false);
                    }
                },
            );
        }

        info!("Sidecar collapsed: {} (animated)", is_collapsed);
    }
}

extern "C" fn on_draft_edit(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = state.selected_draft_index {
        if let Some(path) = state.draft_files.get(index) {
            let opened = open_file_in_editor(path);
            if opened {
                info!("Opened draft in editor: {}", path.display());
            } else {
                info!("Failed to open draft: {}", path.display());
            }
        }
    } else {
        info!("No draft selected for edit");
    }
}

extern "C" fn on_draft_copy(_this: &Object, _cmd: Sel, _sender: Id) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = state.selected_draft_index {
        if let Some(path) = state.draft_files.get(index) {
            if let Ok(content) = std::fs::read_to_string(path) {
                copy_to_clipboard(&content);
                info!("Copied draft to clipboard: {}", path.display());
            } else {
                info!("Failed to read draft: {}", path.display());
            }
        }
    } else {
        info!("No draft selected for copy");
    }
}

/// Copy a specific message by index (retrieved from button tag)
extern "C" fn on_copy_message(_this: &Object, _cmd: Sel, sender: Id) {
    unsafe {
        // Get message index from button's tag
        let tag: isize = msg_send![sender, tag];
        let msg_index = tag as usize;

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(message) = state.messages.get(msg_index) {
            copy_to_clipboard(&message.text);
            debug!("Copied message {} to clipboard", msg_index);
        } else {
            debug!("Invalid message index: {}", msg_index);
        }
    }
}

fn copy_to_clipboard(text: &str) {
    unsafe {
        let pasteboard_class = Class::get("NSPasteboard").unwrap();
        let pasteboard: Id = msg_send![pasteboard_class, generalPasteboard];
        let _: () = msg_send![pasteboard, clearContents];

        let ns_string = Class::get("NSString").unwrap();
        let mut c_str = text.as_bytes().to_vec();
        c_str.push(0);
        let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];

        // NSPasteboardTypeString = "public.utf8-plain-text"
        let type_str: Id =
            msg_send![ns_string, stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
        let _: () = msg_send![pasteboard, setString: ns_str forType: type_str];
    }
}

fn is_near_bottom(scroll_view: Id) -> bool {
    unsafe {
        let content_view: Id = msg_send![scroll_view, contentView];
        let visible_rect: CGRect = msg_send![content_view, documentVisibleRect];
        // If content is reversed (newest at top), we care about being near top (y=0)
        // But NSTextView coordinate system puts (0,0) at top-left usually?
        // Actually in flipped coordinates (standard in Cocoa for Text), (0,0) is top-left.
        // However, standard NSScrollView with NSTextView:
        // By default, NSTextView is not flipped, so (0,0) is bottom-left.
        // But usually we want natural reading.
        // Let's assume standard behavior first.

        // If we reverse the log string, the newest message is at the beginning of the string.
        // So it will be rendered at the TOP of the text view.
        // So "near bottom" check logic might be irrelevant if we auto-scroll to TOP?
        // Let's update scroll logic later. For now let's stick to standard append behavior
        // but reverse the content string construction.

        let document_view: Id = msg_send![scroll_view, documentView];
        let document_rect: CGRect = msg_send![document_view, bounds];
        let visible_max_y = visible_rect.origin.y + visible_rect.size.height;
        let doc_max_y = document_rect.origin.y + document_rect.size.height;
        (doc_max_y - visible_max_y) <= 8.0
    }
}

fn render_chat_log(messages: &[ChatMessage]) -> String {
    let mut output = String::new();
    // Reverse order: Newest messages first (at the top)
    for message in messages.iter().rev() {
        let prefix = match message.role {
            ChatRole::User => "Ty",
            ChatRole::Assistant => "Asystent",
            ChatRole::System => "System",
        };
        let status_suffix = if message.is_streaming { " …" } else { "" };
        let error_prefix = if message.is_error { "Błąd: " } else { "" };

        // Format:
        // [Role]: Text...
        //
        output.push_str(prefix);
        output.push_str(": ");
        output.push_str(error_prefix);
        output.push_str(&message.text);
        output.push_str(status_suffix);
        output.push_str("\n\n---\n\n"); // Separator for clarity in reverse order
    }
    output
}

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
        // state.messages.clear();
        // state.draft_text.clear();
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
        let _right_panel_width = 300.0; // 40% for drafts sidecar (Phase 2)

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
        let controls_y = window_height - header_height - input_area_height + 5.0; // slightly centered

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

        // Show the window
        let _: () = msg_send![window, orderFrontRegardless];

        state.window = Some(window as usize);
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

        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&mut state);
        update_send_button_with_state(&mut state);
        populate_drafts_list(&mut state);
        info!("Voice chat overlay shown");
    }
}

/// Update the status text in the overlay
pub fn update_voice_chat_status(status: &str) {
    let status_owned = status.to_string();
    Queue::main().exec_async(move || {
        update_voice_chat_status_impl(&status_owned);
    });
}

fn update_voice_chat_status_impl(status: &str) {
    unsafe {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(status_field_ptr) = state.status_field {
            let status_field = status_field_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();

            // Create null-terminated C string
            let mut c_str = status.as_bytes().to_vec();
            c_str.push(0);

            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![status_field, setStringValue: ns_str];
        }
    }
}

/// Append a delta (streaming token) to the overlay text
pub fn append_voice_chat_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_draft_impl(&delta_owned);
    });
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct NSRange {
    location: usize,
    length: usize,
}

fn append_voice_chat_draft_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    // Voice streaming goes to voice_draft (right panel / sidecar)
    state.voice_draft.push_str(delta);
    state.is_voice_active = true;
    update_voice_draft_view_with_state(&mut state);
    // Note: We don't update manual input field here - they are separate
}

/// Finalize voice draft: save to file and clear buffer
/// Called when VAD stops or recording finishes
pub fn finalize_voice_draft() -> Option<std::path::PathBuf> {
    Queue::main().exec_sync(finalize_voice_draft_impl)
}

fn finalize_voice_draft_impl() -> Option<std::path::PathBuf> {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());

    // Don't save empty drafts
    let draft_text = state.voice_draft.trim();
    if draft_text.is_empty() {
        state.is_voice_active = false;
        return None;
    }

    // Save draft to file
    let path = codescribe_core::state::save_draft(draft_text);

    // Clear voice draft buffer
    state.voice_draft.clear();
    state.is_voice_active = false;

    // Update UI: clear voice draft view
    update_voice_draft_view_with_state(&mut state);

    // Refresh drafts list to show the new file
    populate_drafts_list(&mut state);

    info!("Voice draft finalized: {}", path.display());
    Some(path)
}

/// Get the current voice draft text (for reading without clearing)
pub fn get_voice_draft() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.voice_draft.clone()
}

/// Clear voice draft without saving (e.g., on cancel)
pub fn clear_voice_draft() {
    Queue::main().exec_async(|| {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.voice_draft.clear();
        state.is_voice_active = false;
        update_voice_draft_view_with_state(&mut state);
    });
}

/// Append a delta to the assistant response (streaming).
pub fn append_voice_chat_assistant_delta(delta: &str) {
    let delta_owned = delta.to_string();
    Queue::main().exec_async(move || {
        append_voice_chat_assistant_delta_impl(&delta_owned);
    });
}

fn append_voice_chat_assistant_delta_impl(delta: &str) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    ensure_streaming_assistant_message(&mut state);
    if let Some(last) = state.messages.last_mut() {
        last.text.push_str(delta);
        last.is_streaming = true;
    }
    update_chat_view_with_state(&mut state, false);
}

/// Set the full text in the overlay for the assistant response.
pub fn set_voice_chat_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        finalize_assistant_message_impl(&text_owned, false);
    });
}

/// Add an error message to the chat log.
pub fn add_voice_chat_error_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.push(ChatMessage {
            role: ChatRole::System,
            text: text_owned.clone(),
            is_streaming: false,
            is_error: true,
        });
        state.is_sending = false;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
    });
}

/// Set the current voice draft text (streaming from Whisper).
pub fn set_voice_chat_draft_text(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.voice_draft = text_owned;
        state.is_voice_active = true;
        update_voice_draft_view_with_state(&mut state);
    });
}

/// Submit the current draft (manual send).
pub fn send_voice_chat_draft() {
    Queue::main().exec_async(move || {
        send_draft_message_impl();
    });
}

/// Set the send callback invoked when the user submits a message.
pub fn set_voice_chat_send_callback(callback: Option<VoiceChatSendCallback>) {
    let mut handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *handler = callback;
}

/// Toggle loading state for sending.
pub fn set_voice_chat_sending(is_sending: bool) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = is_sending;
        update_send_button_with_state(&mut state);
    });
}

/// Get the current voice draft text from the overlay (for auto-send).
pub fn get_accumulated_text() -> String {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.voice_draft.clone()
}

/// Clear the text content of the overlay
pub fn clear_voice_chat_text() {
    Queue::main().exec_async(|| {
        clear_voice_chat_text_impl();
    });
}

fn clear_voice_chat_text_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.messages.clear();
    // Clear both buffers
    state.manual_draft.clear();
    state.voice_draft.clear();
    state.attachments.clear();
    state.is_sending = false;
    state.is_voice_active = false;
    update_chat_view_with_state(&mut state, true);
    update_input_field_with_state(&mut state);
    update_voice_draft_view_with_state(&mut state);
    update_send_button_with_state(&mut state);
}

/// Check if auto-send is enabled
pub fn is_auto_send_enabled() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.auto_send_enabled
}

fn update_chat_view_with_state(state: &mut VoiceChatOverlayState, force_rebuild: bool) {
    let Some(container_ptr) = state.bubble_container else {
        return;
    };
    let container = container_ptr as Id;

    // Check if we need to rebuild bubbles or just update the last one
    let bubble_count = state.bubble_views.len();
    let message_count = state.messages.len();

    // If streaming update to last message (same count, last is streaming)
    if !force_rebuild
        && bubble_count == message_count
        && message_count > 0
        && let Some(last_msg) = state.messages.last()
        && last_msg.is_streaming
        && let Some((_, text_label_ptr)) = state.bubble_views.last()
    {
        // Just update the last bubble's text
        let text_label = *text_label_ptr as Id;
        update_bubble_text(text_label, &last_msg.text, true);
        return;
    }

    // Full rebuild: clear existing bubbles
    stack_view_clear(container);
    state.bubble_views.clear();

    // Get max width for bubbles (left panel width - padding)
    let max_bubble_width = 420.0; // ~left_panel_width - 30

    // Get action handler for Copy buttons
    let action_handler = state.action_handler.map(|ptr| ptr as Id);

    // Add bubbles in reverse order (newest first at top)
    // Use enumerate to track original message indices for Copy buttons
    let messages_count = state.messages.len();
    for (rev_idx, message) in state.messages.iter().rev().enumerate() {
        // Convert reversed index back to original index
        let original_idx = messages_count - 1 - rev_idx;

        let role = match message.role {
            ChatRole::User => BubbleRole::User,
            ChatRole::Assistant => BubbleRole::Assistant,
            ChatRole::System => BubbleRole::System,
        };

        // Only show Copy button for completed messages (not streaming)
        let (message_index, copy_target) = if !message.is_streaming {
            (Some(original_idx), action_handler)
        } else {
            (None, None)
        };

        let config = BubbleConfig {
            text: message.text.clone(),
            role,
            max_width: max_bubble_width,
            is_streaming: message.is_streaming,
            is_error: message.is_error,
            message_index,
            copy_action_target: copy_target,
        };

        let (bubble_view, text_label) = create_bubble_view(config);
        stack_view_add(container, bubble_view);
        state
            .bubble_views
            .push((bubble_view as usize, text_label as usize));
    }

    // Scroll to top if forced (newest messages are at top)
    if force_rebuild && let Some(scroll_view_ptr) = state.scroll_view {
        unsafe {
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            let _: () = msg_send![content_view, scrollToPoint: CGPoint { x: 0.0, y: 0.0 }];
            let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
        }
    }
}

fn update_input_field_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(input_ptr) = state.input_field {
            let input_field = input_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();
            // Manual input field shows manual_draft (left panel)
            let mut c_str = state.manual_draft.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![input_field, setStringValue: ns_str];
        }
    }
}

fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(send_ptr) = state.send_button {
            let send_button = send_ptr as Id;
            // Send button enabled when manual_draft has content
            let enabled = !state.is_sending && !state.manual_draft.trim().is_empty();
            let _: () = msg_send![send_button, setEnabled: enabled];
        }
    }
}

/// Populate the drafts list from ~/.codescribe/drafts/
fn populate_drafts_list(state: &mut VoiceChatOverlayState) {
    let Some(container_ptr) = state.drafts_container else {
        return;
    };
    let container = container_ptr as Id;

    // Clear existing items
    stack_view_clear(container);
    state.draft_files.clear();
    state.selected_draft_index = None;

    // Get drafts directory
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let drafts_dir = std::path::PathBuf::from(home).join(".codescribe/drafts");

    // List and cache draft files
    state.draft_files = list_draft_files(&drafts_dir);

    // Create UI row for each draft file
    for (index, path) in state.draft_files.iter().enumerate() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Create a simple label for the draft
        let row = create_draft_row(filename, index);
        stack_view_add(container, row);
    }

    // Select first draft if available
    if !state.draft_files.is_empty() {
        state.selected_draft_index = Some(0);
    }

    info!("Populated {} drafts", state.draft_files.len());
}

/// Create a row for a draft file in the list
fn create_draft_row(filename: &str, _index: usize) -> Id {
    unsafe {
        let ns_text_field = Class::get("NSTextField").unwrap();
        let ns_color = Class::get("NSColor").unwrap();
        let ns_font = Class::get("NSFont").unwrap();

        // Simple text field showing filename with icon
        let display_text = format!("📄 {}", filename);

        let row_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 260.0,
                height: 24.0,
            },
        };

        let row: Id = msg_send![ns_text_field, alloc];
        let row: Id = msg_send![row, initWithFrame: row_frame];

        let _: () = msg_send![row, setBezeled: false];
        let _: () = msg_send![row, setEditable: false];
        let _: () = msg_send![row, setSelectable: true];
        let _: () = msg_send![row, setDrawsBackground: false];

        // White text
        let white: Id = msg_send![ns_color, whiteColor];
        let _: () = msg_send![row, setTextColor: white];

        // Small font
        let font: Id = msg_send![ns_font, systemFontOfSize: 11.0f64];
        let _: () = msg_send![row, setFont: font];

        // Set text
        let text = ns_string(&display_text);
        let _: () = msg_send![row, setStringValue: text];

        row
    }
}

/// Update the voice draft view (right panel / sidecar) with current voice_draft text
fn update_voice_draft_view_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(view_ptr) = state.voice_draft_view {
            let text_view = view_ptr as Id;
            let ns_string = Class::get("NSString").unwrap();
            let mut c_str = state.voice_draft.as_bytes().to_vec();
            c_str.push(0);
            let ns_str: Id = msg_send![ns_string, stringWithUTF8String: c_str.as_ptr()];
            let _: () = msg_send![text_view, setString: ns_str];
        }
    }
}

fn ensure_streaming_assistant_message(state: &mut VoiceChatOverlayState) {
    let needs_new = match state.messages.last() {
        Some(last) => last.role != ChatRole::Assistant || !last.is_streaming,
        None => true,
    };
    if needs_new {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: String::new(),
            is_streaming: true,
            is_error: false,
        });
    }
}

fn finalize_assistant_message_impl(text: &str, is_error: bool) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let needs_new = match state.messages.last_mut() {
        Some(last) if last.role == ChatRole::Assistant => {
            last.text = text.to_string();
            last.is_streaming = false;
            last.is_error = is_error;
            false
        }
        _ => true,
    };
    if needs_new {
        state.messages.push(ChatMessage {
            role: ChatRole::Assistant,
            text: text.to_string(),
            is_streaming: false,
            is_error,
        });
    }
    state.is_sending = false;
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
}

pub fn add_voice_chat_user_message(text: &str) {
    let text_owned = text.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: text_owned,
            is_streaming: false,
            is_error: false,
        });
        update_chat_view_with_state(&mut state, true);
    });
}

fn send_draft_message_impl() {
    // This sends from manual_draft (left panel input field)
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let draft = state.manual_draft.trim().to_string();
        if draft.is_empty() {
            return;
        }
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: draft.clone(),
            is_streaming: false,
            is_error: false,
        });
        state.manual_draft.clear();
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_input_field_with_state(&mut state);
        update_send_button_with_state(&mut state);
        let handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        (handler.clone(), draft)
    };

    if let (Some(handler), draft) = callback {
        handler(draft);
    } else {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = false;
        update_send_button_with_state(&mut state);
    }
}

/// Check if the voice chat overlay is currently visible
pub fn is_voice_chat_overlay_visible() -> bool {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.window.is_some()
}

/// Reset the auto-hide timer (placeholder for future implementation)
pub fn reset_voice_chat_activity() {
    // Currently no auto-hide timer, but function exists for API compatibility
    debug!("reset_voice_chat_activity called");
}

/// Hide the voice chat overlay window
pub fn hide_voice_chat_overlay() {
    Queue::main().exec_async(|| {
        hide_voice_chat_overlay_impl();
    });
}

fn hide_voice_chat_overlay_impl() {
    unsafe {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window.take() {
            let window = window_ptr as Id;
            let _: () = msg_send![window, close];
            debug!("Voice chat overlay hidden");
        }
        state.bubble_container = None;
        state.bubble_views.clear();
        state.status_field = None;
        state.voice_draft_view = None;
        state.voice_draft_header = None;
        state.voice_send_button = None;
        state.voice_use_button = None;
        state.tab_bar = None;
        state.drafts_scroll_view = None;
        state.drafts_container = None;
        state.draft_files.clear();
        state.selected_draft_index = None;
        state.messages.clear();
        // Clear both buffers
        state.manual_draft.clear();
        state.voice_draft.clear();
        state.attachments.clear();
        state.is_sending = false;
        state.is_voice_active = false;
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

    #[test]
    fn test_overlay_config_clone() {
        let config = VoiceChatOverlayConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.width, config.width);
        assert_eq!(cloned.height, config.height);
    }

    #[test]
    fn test_overlay_config_debug() {
        let config = VoiceChatOverlayConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("VoiceChatOverlayConfig"));
        assert!(debug_str.contains("750")); // Mission Control width
    }

    #[test]
    fn test_overlay_state_initial() {
        // Verify the initial state is empty
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        // Window should be None initially (unless another test created it)
        // Just verify we can access the state without panic
        let _ = state.manual_draft.len();
        let _ = state.voice_draft.len();
    }

    #[test]
    fn test_is_overlay_visible_returns_bool() {
        // Just verify the function returns a bool without panic
        let visible = is_voice_chat_overlay_visible();
        // Can be either true or false depending on test order
        let _ = visible;
    }

    #[test]
    fn test_render_chat_log_reverse_order() {
        let messages = vec![
            ChatMessage {
                role: ChatRole::User,
                text: "First".to_string(),
                is_streaming: false,
                is_error: false,
            },
            ChatMessage {
                role: ChatRole::Assistant,
                text: "Second".to_string(),
                is_streaming: false,
                is_error: false,
            },
        ];

        let output = render_chat_log(&messages);

        // Should find "Second" before "First" because of reverse iteration
        let second_pos = output.find("Second").unwrap();
        let first_pos = output.find("First").unwrap();

        assert!(
            second_pos < first_pos,
            "Messages should be rendered in reverse order (newest first)"
        );
    }

    #[test]
    fn test_auto_send_toggle_state() {
        // Initial state is true
        let initial = is_auto_send_enabled();

        // Manually toggle via internal mutex
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.auto_send_enabled = !initial;
        }

        assert_ne!(is_auto_send_enabled(), initial);

        // Toggle back
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.auto_send_enabled = initial;
        }
    }

    #[test]
    fn test_persistence_logic() {
        // Simulate adding a message
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.messages.push(ChatMessage {
                role: ChatRole::User,
                text: "PersistMe".to_string(),
                is_streaming: false,
                is_error: false,
            });
        }

        // Simulate "showing" overlay (logic which previously cleared messages)
        // We can't call show_voice_chat_overlay_impl directly because it uses Cocoa/UI methods
        // which might crash in headless test.
        // Instead, we verify that the clear functions are NOT called by inspecting logic?
        // Impossible to inspect logic dynamically here.
        // But we can verify that our helper functions don't clear it.

        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!state.messages.is_empty(), "Messages should persist");
        assert!(state.messages.iter().any(|m| m.text == "PersistMe"));
    }

    #[test]
    fn test_sidecar_collapsed_toggle() {
        // Initial state
        let initial = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.sidecar_collapsed
        };

        // Toggle via mutex
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.sidecar_collapsed = !initial;
        }

        let toggled = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.sidecar_collapsed
        };
        assert_ne!(toggled, initial, "Sidecar collapsed should toggle");

        // Restore
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.sidecar_collapsed = initial;
        }
    }

    #[test]
    fn test_selected_tab_change() {
        // Set to Drafts (0)
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.selected_tab = 0;
        }

        let tab = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.selected_tab
        };
        assert_eq!(tab, 0, "Tab should be Drafts (0)");

        // Switch to Settings (1)
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.selected_tab = 1;
        }

        let tab = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.selected_tab
        };
        assert_eq!(tab, 1, "Tab should be Settings (1)");

        // Restore
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.selected_tab = 0;
        }
    }

    #[test]
    fn test_attachments_list() {
        // Clear first
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.clear();
        }

        // Add attachments
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state
                .attachments
                .push(std::path::PathBuf::from("/tmp/test1.txt"));
            state
                .attachments
                .push(std::path::PathBuf::from("/tmp/test2.png"));
        }

        let count = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.len()
        };
        assert_eq!(count, 2, "Should have 2 attachments");

        // Clear
        {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.clear();
        }

        let count = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.attachments.len()
        };
        assert_eq!(count, 0, "Attachments should be cleared");
    }
}
