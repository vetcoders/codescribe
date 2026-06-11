//! Drawer tab: transcription/thread cards, filtering, rendering and loading.

use super::*;

/// Refresh drawer entries from disk
pub fn refresh_drawer() {
    Queue::main().exec_async(|| {
        refresh_drawer_impl();
    });
}

/// Filter drawer entries by query (reloads from disk)
pub fn filter_drawer(query: &str) {
    let query_owned = query.to_string();
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.drawer_entries = load_drawer_entries_with_query(&query_owned);
        render_drawer_entries(&mut state, &query_owned);
    });
}

pub fn refresh_drawer_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.favorites = load_favorites_from_disk();
    let query = drawer_query_from_state(&state);
    state.drawer_entries = load_drawer_entries();
    render_drawer_entries(&mut state, &query);
}

pub fn handle_card_copy(index: usize) {
    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if is_drawer_unavailable_placeholder(entry) {
            return;
        }
        match &entry.source {
            DrawerEntrySource::Thread { id } => {
                if let Ok(store) = ThreadStore::new() {
                    if let Ok(thread) = store.load_thread(id) {
                        copy_to_clipboard(&thread_markdown_for_copy(&thread));
                        return;
                    }
                    if let Ok(raw) = std::fs::read_to_string(&entry.path) {
                        copy_to_clipboard(&raw);
                    }
                }
            }
            DrawerEntrySource::LegacyFile => {
                if let Ok(contents) = std::fs::read_to_string(&entry.path) {
                    copy_to_clipboard(&contents);
                }
            }
        }
    }
}

pub fn handle_card_restore(index: usize) {
    let thread = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.drawer_entries.get(index) else {
            return;
        };
        if is_drawer_unavailable_placeholder(entry) {
            return;
        }
        let DrawerEntrySource::Thread { id } = &entry.source else {
            return;
        };
        let Ok(store) = ThreadStore::new() else {
            warn!("Failed to initialize ThreadStore for restore");
            return;
        };
        match store.load_thread(id) {
            Ok(thread) => thread,
            Err(error) => {
                warn!("Failed to restore thread {id}: {error}");
                return;
            }
        }
    };

    let title = thread.title.trim().to_string();
    let mut restored_messages = thread_messages_for_restore(&thread);
    if restored_messages.is_empty() {
        restored_messages.push(ChatMessage {
            role: ChatRole::System,
            text: "Restored thread has no messages.".to_string(),
            is_streaming: false,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode_label(transcription_mode_from_thread_mode(&thread.mode)).to_string()),
        });
    }

    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.messages = restored_messages;
    state.active_user_stream_index = None;
    state.active_assistant_stream_index = None;
    state.is_sending = false;
    state.manual_draft.clear();
    state.attachments.clear();
    state.attachments_last_sent = None;
    clear_agent_thinking_state(&mut state);
    update_active_tab_locked(&mut state, Tab::Agent);
    update_chat_view_with_state(&mut state, true);
    update_send_button_with_state(&mut state);
    let title = if title.is_empty() {
        "Restored thread".to_string()
    } else {
        format!("Restored: {title}")
    };
    state.status_base_text = title;
    state.status_text = compose_runtime_status_text(
        &state.status_base_text,
        state.is_agent_degraded,
        state.runtime_degraded_reason.as_deref(),
    );
    state.status_kind = UiStatus::Idle;
    apply_status_pill(&state);
    let _ = crate::tray::update_tray_status(state.status_kind.to_tray());
}

pub fn handle_card_edit(index: usize) {
    let (path, window_usize) = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let path = state.drawer_entries.get(index).map(|e| e.path.clone());
        (path, state.window)
    };

    let Some(path) = path else {
        return;
    };
    if path.as_os_str().is_empty() {
        return;
    }

    tracing::info!("Drawer Edit clicked: {}", path.display());
    let ok = open_file_in_editor(&path);
    if !ok {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&path)
                .status();
        }
        tracing::warn!("Drawer Edit failed to open: {}", path.display());
        return;
    }

    // UX: briefly hide the overlay so the editor is visible immediately.
    // Then only bring it back if CodeScribe is still the active app.
    #[cfg(target_os = "macos")]
    if let Some(window_usize) = window_usize {
        unsafe {
            crate::ui_helpers::window_hide(window_usize as Id);
        }

        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(750));

            Queue::main().exec_async(move || {
                let still_same_window = {
                    let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    state.window == Some(window_usize)
                };
                if !still_same_window {
                    return;
                }

                let is_active = unsafe {
                    let ns_running_app = match Class::get("NSRunningApplication") {
                        Some(c) => c,
                        None => return,
                    };
                    let current: Id = msg_send![ns_running_app, currentApplication];
                    if current.is_null() {
                        return;
                    }
                    let active: bool = msg_send![current, isActive];
                    active
                };

                // Restore floating level and show only if CodeScribe is active.
                unsafe {
                    let window = window_usize as Id;
                    let _: () = msg_send![
                        window,
                        setLevel: crate::ui_helpers::NS_FLOATING_WINDOW_LEVEL
                    ];
                }
                if is_active {
                    unsafe {
                        crate::ui_helpers::window_show(window_usize as Id);
                    }
                }
            });
        });
    }
}

pub fn handle_card_delete(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = state.drawer_entries.get(index) {
        if is_drawer_unavailable_placeholder(entry) {
            return;
        }
        let favorite_key = drawer_entry_favorite_key(entry);
        match &entry.source {
            DrawerEntrySource::Thread { id } => {
                if let Ok(store) = ThreadStore::new() {
                    if let Err(err) = store.delete_thread(id) {
                        warn!("Failed to delete thread {id}: {err}");
                    }
                } else if let Err(err) = std::fs::remove_file(&entry.path) {
                    warn!(
                        "Failed to delete thread fallback {}: {}",
                        entry.path.display(),
                        err
                    );
                }
            }
            DrawerEntrySource::LegacyFile => {
                if let Err(err) = std::fs::remove_file(&entry.path) {
                    warn!("Failed to delete {}: {}", entry.path.display(), err);
                }
            }
        }
        state.favorites.remove(&favorite_key);
        save_favorites_to_disk(&state.favorites);
    }
    state.favorites = load_favorites_from_disk();
    let query = drawer_query_from_state(&state);
    state.drawer_entries = load_drawer_entries_with_query(&query);
    render_drawer_entries(&mut state, &query);
}

pub fn handle_card_favorite(index: usize) {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = state.drawer_entries.get_mut(index) else {
        return;
    };
    if is_drawer_unavailable_placeholder(entry) {
        return;
    }

    entry.is_favorite = !entry.is_favorite;
    let is_favorite = entry.is_favorite;
    let key = drawer_entry_favorite_key(entry);
    let thread_id = match &entry.source {
        DrawerEntrySource::Thread { id } => Some(id.clone()),
        DrawerEntrySource::LegacyFile => None,
    };

    if is_favorite {
        state.favorites.insert(key);
    } else {
        state.favorites.remove(&key);
    }
    save_favorites_to_disk(&state.favorites);

    if let Some(id) = thread_id
        && let Ok(store) = ThreadStore::new()
        && let Err(err) = store.set_thread_favorite(&id, is_favorite)
    {
        warn!("Failed to update thread favorite {id}: {err}");
    }
    update_favorites_button_with_state(&mut state);
    let query = drawer_query_from_state(&state);
    render_drawer_entries(&mut state, &query);
}

pub fn toggle_drawer_favorites_only_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.drawer_favorites_only = !state.drawer_favorites_only;

    // Jump to Drawer (this feature is Drawer-scoped).
    update_active_tab_locked(&mut state, Tab::Drawer);

    update_favorites_button_with_state(&mut state);

    let query = drawer_query_from_state(&state);
    render_drawer_entries(&mut state, &query);
}

pub fn update_favorites_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(btn_ptr) = state.favorites_button else {
            return;
        };
        let btn = btn_ptr as Id;
        let symbol = if state.drawer_favorites_only {
            "heart.fill"
        } else {
            "heart"
        };
        let has_symbol = set_button_symbol(btn, symbol);
        if !has_symbol {
            let title = if state.drawer_favorites_only {
                "♥"
            } else {
                "♡"
            };
            let title = ns_string(title);
            let _: () = msg_send![btn, setTitle: title];
        }
    }
}

pub fn drawer_query_from_state(state: &VoiceChatOverlayState) -> String {
    state
        .search_field
        .map(|field| unsafe { get_text_field_string(field as Id) })
        .unwrap_or_default()
}

pub fn drawer_entry_matches_query(entry: &DrawerEntry, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    // Path pollution guard: do NOT push entry.path (absolute) into the haystack.
    // Every ThreadStore entry lives under `~/.codescribe/` or `~/Library/Application
    // Support/CodeScribe/`, so any query overlapping the app data dir name (e.g.
    // "codescribe", "thread", "users") would match all entries via leaked path
    // components. Operator flagged 2026-05-24 ("threadstore, wyszukiwanie codescribe
    // nie odfiltrowuje nic"). Keep file_name (local, useful for legacy file dates)
    // and thread id (specific to the entry), drop absolute path.
    let file_name_str = entry
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let mut haystack = String::with_capacity(
        file_name_str.len() + entry.preview.len() + entry.search_corpus.len() + 64,
    );
    haystack.push_str(entry_type_label(entry));
    haystack.push(' ');
    haystack.push_str(mode_label(entry.mode));
    haystack.push(' ');
    if !file_name_str.is_empty() {
        haystack.push_str(file_name_str);
        haystack.push(' ');
    }
    if let DrawerEntrySource::Thread { id } = &entry.source {
        haystack.push_str(id);
        haystack.push(' ');
    }
    haystack.push_str(&entry.preview);
    haystack.push(' ');
    haystack.push_str(&entry.search_corpus);
    haystack.to_lowercase().contains(query_lower)
}

pub fn filtered_drawer_entries<'a>(
    state: &'a VoiceChatOverlayState,
    query: &str,
) -> Vec<(usize, &'a DrawerEntry)> {
    let filter = query.trim().to_lowercase();
    let mut out = Vec::new();
    for (index, entry) in state.drawer_entries.iter().enumerate() {
        if state.drawer_favorites_only && !entry.is_favorite {
            continue;
        }
        if !drawer_entry_matches_query(entry, &filter) {
            continue;
        }
        out.push((index, entry));
    }
    out
}

pub fn drawer_top_scroll_y(document_height: f64, clip_height: f64, is_flipped: bool) -> f64 {
    if is_flipped {
        0.0
    } else {
        (document_height - clip_height).max(0.0)
    }
}

pub fn render_drawer_entries(state: &mut VoiceChatOverlayState, query: &str) {
    unsafe {
        let Some(container_ptr) = state.drawer_container else {
            return;
        };
        let container = container_ptr as Id;
        stack_view_clear(container);

        let visible = filtered_drawer_entries(state, query);
        for (index, entry) in visible.iter() {
            let card = create_drawer_card(entry, *index, state.action_handler, query);
            stack_view_add(container, card);
        }

        if visible.is_empty() {
            let frame: CGRect = msg_send![container, frame];
            let empty_state = create_drawer_empty_state(frame.size.width, state.action_handler);
            stack_view_add(container, empty_state);
        }

        let _: () = msg_send![container, setNeedsLayout: true];
        let _: () = msg_send![container, layoutSubtreeIfNeeded];

        // Keep the scroll document height in sync with its arranged subviews; otherwise the
        // scroll view can end up showing an empty area (looks like the drawer "does nothing").
        if let Some(scroll_view_ptr) = state.drawer_scroll_view {
            let scroll_view = scroll_view_ptr as Id;
            let content_view: Id = msg_send![scroll_view, contentView];
            if !content_view.is_null() {
                let fitting: CGSize = msg_send![container, fittingSize];
                let frame: CGRect = msg_send![container, frame];
                let clip_bounds: CGRect = msg_send![content_view, bounds];
                let document_width = frame.size.width.max(clip_bounds.size.width).max(1.0);
                let document_height = fitting
                    .height
                    .ceil()
                    .max(frame.size.height)
                    .max(clip_bounds.size.height)
                    .max(1.0);
                let new_size = CGSize::new(document_width, document_height);
                let _: () = msg_send![container, setFrameSize: new_size];
                let _: () = msg_send![container, setNeedsLayout: true];
                let _: () = msg_send![container, layoutSubtreeIfNeeded];

                // Scroll to the visual top on every refresh/filter. NSStackView is not flipped,
                // so its top is `document_height - clip_height`, not y=0.
                let is_flipped: bool = msg_send![container, isFlipped];
                let top_y =
                    drawer_top_scroll_y(document_height, clip_bounds.size.height, is_flipped);
                let _: () = msg_send![content_view, scrollToPoint: CGPoint::new(0.0, top_y)];
                let _: () = msg_send![scroll_view, reflectScrolledClipView: content_view];
                let _: () = msg_send![scroll_view, tile];
                let _: () = msg_send![container, setNeedsDisplay: true];
                let _: () = msg_send![scroll_view, setNeedsDisplay: true];
            }
        }
    }
}

pub fn create_drawer_empty_state(width: f64, handler: Option<usize>) -> Id {
    fn overlay_hotkey_shortcuts_tooltip() -> String {
        let (hold, toggle) =
            crate::ui::voice_chat::shortcuts_lines(crate::os::hotkeys::ModeHotkeyBindings::load());
        format!("{hold}\n{toggle}")
    }

    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let frame = CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width.max(240.0), ui_tokens::EMPTY_STATE_HEIGHT),
        );
        let view: Id = msg_send![ns_view, alloc];
        let view: Id = msg_send![view, initWithFrame: frame];
        let _: () = msg_send![view, setWantsLayer: true];
        let layer: Id = msg_send![view, layer];
        if !layer.is_null() {
            let bg = ui_colors::empty_state_bg();
            let cg: Id = msg_send![bg, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: cg];
            apply_tafla_surface(layer, true);
        }

        let pad = ui_tokens::EDGE_PADDING;
        let title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 36.0),
                &CGSize::new(frame.size.width - pad * 2.0, 20.0),
            ),
            text: "No items yet".to_string(),
            font_size: ui_tokens::BODY_FONT_SIZE,
            bold: true,
            text_color: color_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, title);

        let body = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 58.0),
                &CGSize::new(frame.size.width - pad * 2.0, 18.0),
            ),
            text: "Start recording to capture a transcript.".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, body);

        let body2 = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(pad, frame.size.height - 76.0),
                &CGSize::new(frame.size.width - pad * 2.0, 18.0),
            ),
            text: "Need permissions or hotkeys? Open Settings.".to_string(),
            font_size: ui_tokens::SMALL_FONT_SIZE,
            bold: false,
            text_color: color_secondary_label(),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(view, body2);

        let button_h = ui_tokens::EMPTY_STATE_BUTTON_HEIGHT;
        let button_w = ui_tokens::EMPTY_STATE_BUTTON_WIDTH;
        let button_gap = ui_tokens::EMPTY_STATE_BUTTON_GAP;
        let row_w = button_w * 2.0 + button_gap;
        let row_x = ((frame.size.width - row_w) / 2.0).max(pad);

        let start_button = create_button(
            CGRect::new(&CGPoint::new(row_x, pad), &CGSize::new(button_w, button_h)),
            "Start recording",
            button_style::ROUNDED,
        );
        let overlay_button = create_button(
            CGRect::new(
                &CGPoint::new(row_x + button_w + button_gap, pad),
                &CGSize::new(button_w, button_h),
            ),
            "Open Settings",
            button_style::ROUNDED,
        );

        if let Some(handler_ptr) = handler {
            let handler_id = handler_ptr as Id;
            button_set_action(start_button, handler_id, sel!(onStartRecording:));
            button_set_action(overlay_button, handler_id, sel!(onTabSettings:));
        }

        let shortcuts_tooltip = overlay_hotkey_shortcuts_tooltip();
        set_tooltip(start_button, &shortcuts_tooltip);
        set_tooltip(
            overlay_button,
            "Open Settings (permissions, hotkeys, and runtime services)",
        );
        add_subview(view, start_button);
        add_subview(view, overlay_button);

        view
    }
}

pub fn create_drawer_card(
    entry: &DrawerEntry,
    index: usize,
    handler: Option<usize>,
    query: &str,
) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(410.0, 120.0),
        );
        let title = format!(
            "{}  {}",
            entry_type_label(entry),
            format_relative_time(entry.timestamp)
        );
        let subtitle = drawer_entry_subtitle(entry);
        let preview = entry.preview.clone();
        let card = create_card_view(frame, &title, &subtitle, &preview);
        // Highlight matching query text in the preview field (last NSTextField subview).
        if !query.is_empty() {
            let subviews: Id = msg_send![card, subviews];
            let count: usize = msg_send![subviews, count];
            // The preview field is typically the 3rd text field added (index 2).
            // Walk subviews in reverse to find it (last NSTextField before action buttons).
            for i in (0..count).rev() {
                let subview: Id = msg_send![subviews, objectAtIndex: i];
                let ns_text_field = Class::get("NSTextField").unwrap();
                let is_text_field: bool = msg_send![subview, isKindOfClass: ns_text_field];
                if is_text_field {
                    apply_search_highlight(subview, &preview, query);
                    break;
                }
            }
        }

        let actions_container: Id = msg_send![ns_view, alloc];
        let actions_frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(12.0, 8.0),
            &core_graphics::geometry::CGSize::new(386.0, 20.0),
        );
        let actions_container: Id = msg_send![actions_container, initWithFrame: actions_frame];

        let button_titles = ["Restore", "Copy", "Edit", "Delete"];
        let button_actions = [
            sel!(onCardRestore:),
            sel!(onCardCopy:),
            sel!(onCardEdit:),
            sel!(onCardDelete:),
        ];
        for (idx, title) in button_titles.iter().enumerate() {
            let button_width = if *title == "Restore" { 76.0 } else { 62.0 };
            let button = crate::ui_helpers::create_button(
                core_graphics::geometry::CGRect::new(
                    &CGPoint::new((idx as f64) * 74.0, 0.0),
                    &core_graphics::geometry::CGSize::new(button_width, 20.0),
                ),
                title,
                crate::ui_helpers::button_style::ROUNDED,
            );
            let supports_control_size: bool =
                msg_send![button, respondsToSelector: sel!(setControlSize:)];
            if supports_control_size {
                let _: () = msg_send![button, setControlSize: 1_isize]; // NSSmallControlSize
            }
            if let Some(handler) = handler {
                crate::ui_helpers::button_set_action(button, handler as Id, button_actions[idx]);
            }
            let _: () = msg_send![button, setTag: index as isize];
            let _: () = msg_send![actions_container, addSubview: button];
        }

        let favorite = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(310.0, 0.0),
                &core_graphics::geometry::CGSize::new(28.0, 20.0),
            ),
            "",
            crate::ui_helpers::button_style::INLINE,
        );
        let fav_symbol = if entry.is_favorite {
            "heart.fill"
        } else {
            "heart"
        };
        let _ = set_button_symbol(favorite, fav_symbol);
        crate::ui_helpers::style_toolbar_icon_button(favorite);
        let supports_control_size: bool =
            msg_send![favorite, respondsToSelector: sel!(setControlSize:)];
        if supports_control_size {
            let _: () = msg_send![favorite, setControlSize: 1_isize];
        }
        if let Some(handler) = handler {
            crate::ui_helpers::button_set_action(favorite, handler as Id, sel!(onCardFavorite:));
        }
        set_tooltip(favorite, "Favorite");
        let _: () = msg_send![favorite, setTag: index as isize];
        let _: () = msg_send![actions_container, addSubview: favorite];

        let _: () = msg_send![card, addSubview: actions_container];
        card
    }
}

/// NSRange for Objective-C attributed string APIs.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct NSRange {
    location: usize,
    length: usize,
}

/// Apply search-term highlighting to a text field by bolding matching ranges.
///
/// Uses `char_indices()` to safely iterate over Unicode characters, then maps
/// character offsets to UTF-16 code unit counts for `NSRange` (Cocoa convention).
pub unsafe fn apply_search_highlight(field: Id, text: &str, query: &str) {
    let ns_mut_attr = Class::get("NSMutableAttributedString").unwrap();
    let ns_font_cls = Class::get("NSFont").unwrap();
    let text_ns = ns_string(text);
    let attr_str: Id = msg_send![ns_mut_attr, alloc];
    let attr_str: Id = msg_send![attr_str, initWithString: text_ns];
    let bold_font: Id = msg_send![ns_font_cls, boldSystemFontOfSize: ui_tokens::BODY_FONT_SIZE];
    let font_key = ns_string("NSFont");
    // Build char-level lowercase for safe matching (no byte-index slicing).
    let text_chars: Vec<char> = text.chars().collect();
    let text_lower: Vec<char> = text_chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let query_lower: Vec<char> = query
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    if query_lower.is_empty() {
        // Always set the plain attributed string to clear stale highlights.
        let _: () = msg_send![field, setAttributedStringValue: attr_str];
        return;
    }
    // Build byte→utf16 offset map at char boundaries for NSRange conversion.
    let mut char_to_utf16: Vec<usize> = Vec::with_capacity(text_chars.len() + 1);
    let mut utf16_pos: usize = 0;
    for ch in &text_chars {
        char_to_utf16.push(utf16_pos);
        utf16_pos += ch.len_utf16();
    }
    char_to_utf16.push(utf16_pos); // sentinel for end
    // Slide through char-level arrays to find matches.
    let mut i = 0;
    while i + query_lower.len() <= text_lower.len() {
        if text_lower[i..i + query_lower.len()] == query_lower[..] {
            let range = NSRange {
                location: char_to_utf16[i],
                length: char_to_utf16[i + query_lower.len()] - char_to_utf16[i],
            };
            let _: () = msg_send![attr_str, addAttribute: font_key value: bold_font range: range];
            let highlight = ui_colors::search_highlight_bg();
            let bg_key = ns_string("NSBackgroundColor");
            let _: () = msg_send![attr_str, addAttribute: bg_key value: highlight range: range];
            i += query_lower.len();
        } else {
            i += 1;
        }
    }
    let _: () = msg_send![field, setAttributedStringValue: attr_str];
}

pub fn entry_type_label(entry: &DrawerEntry) -> &'static str {
    if is_drawer_unavailable_placeholder(entry) {
        return "Warning";
    }
    match entry.source {
        DrawerEntrySource::Thread { .. } => "ThreadStore",
        DrawerEntrySource::LegacyFile => {
            if entry.is_ai_formatted {
                "Legacy AI"
            } else {
                "Legacy Raw"
            }
        }
    }
}

pub fn drawer_entry_source_label(entry: &DrawerEntry) -> String {
    if is_drawer_unavailable_placeholder(entry) {
        return "ThreadStore".to_string();
    }
    match entry.source {
        DrawerEntrySource::Thread { .. } => {
            if entry.path.exists() {
                "ThreadStore".to_string()
            } else {
                "ThreadStore (index-only)".to_string()
            }
        }
        DrawerEntrySource::LegacyFile => "Legacy transcript file".to_string(),
    }
}

pub fn drawer_entry_subtitle(entry: &DrawerEntry) -> String {
    if is_drawer_unavailable_placeholder(entry) {
        return "Shift/Cmd • ThreadStore • unavailable".to_string();
    }
    let source_label = drawer_entry_source_label(entry);
    match &entry.source {
        DrawerEntrySource::Thread { id } => {
            format!(
                "{} • {} • thread:{id}",
                mode_label(entry.mode),
                source_label
            )
        }
        DrawerEntrySource::LegacyFile => {
            format!(
                "{} • {} • {}",
                mode_label(entry.mode),
                source_label,
                entry.path.display()
            )
        }
    }
}

pub fn mode_label(mode: TranscriptionMode) -> &'static str {
    match mode {
        TranscriptionMode::Hold => "Ctrl+Hold",
        TranscriptionMode::Assistive => "Shift/Cmd",
        TranscriptionMode::Toggle => "Toggle",
        TranscriptionMode::Conversation => "Moshi",
    }
}

pub fn format_relative_time(timestamp: SystemTime) -> String {
    let now = SystemTime::now();
    if let Ok(duration) = now.duration_since(timestamp) {
        let minutes = duration.as_secs() / 60;
        if minutes < 60 {
            return format!("{} min", minutes.max(1));
        }
        let hours = minutes / 60;
        if hours < 24 {
            return format!("{} h", hours);
        }
        let days = hours / 24;
        return format!("{} d", days);
    }
    "just now".to_string()
}

pub fn load_drawer_entries() -> Vec<DrawerEntry> {
    load_drawer_entries_with_query("")
}

pub fn load_drawer_entries_with_query(query: &str) -> Vec<DrawerEntry> {
    let favorites = load_favorites_from_disk();
    let mut entries = load_thread_drawer_entries(&favorites);
    entries.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

    let query_lower = query.trim().to_ascii_lowercase();
    if !query_lower.is_empty() {
        entries.retain(|entry| drawer_entry_matches_query(entry, &query_lower));
    }

    entries
}

pub fn thread_history_unavailable_drawer_entry() -> DrawerEntry {
    DrawerEntry {
        source: DrawerEntrySource::LegacyFile,
        path: PathBuf::from(""),
        timestamp: SystemTime::now(),
        mode: TranscriptionMode::Assistive,
        preview: "Thread history unavailable — storage error".to_string(),
        search_corpus: "thread history unavailable storage error".to_string(),
        is_ai_formatted: false,
        is_favorite: false,
    }
}

pub fn is_drawer_unavailable_placeholder(entry: &DrawerEntry) -> bool {
    matches!(entry.source, DrawerEntrySource::LegacyFile) && entry.path.as_os_str().is_empty()
}

pub fn load_thread_drawer_entries(favorites: &HashSet<String>) -> Vec<DrawerEntry> {
    let Ok(store) = ThreadStore::new() else {
        warn!("Drawer: failed to open ThreadStore; drawer entries unavailable");
        return vec![thread_history_unavailable_drawer_entry()];
    };
    let Ok(index) = ThreadIndex::load_or_create(store.threads_dir()) else {
        warn!("Drawer: failed to load ThreadIndex; drawer entries unavailable");
        return vec![thread_history_unavailable_drawer_entry()];
    };

    index
        .list(None)
        .into_iter()
        .map(|summary| {
            let id = summary.id.clone();
            let source = DrawerEntrySource::Thread { id: id.clone() };
            let favorite_key = format!("thread:{id}");
            let mut preview = summary
                .latest_note
                .as_deref()
                .or(summary.latest_message.as_deref())
                .or(summary.summary.as_deref())
                .unwrap_or(summary.title.as_str())
                .to_string();
            let mut search_corpus = summary.search_text.clone();
            if (search_corpus.trim().is_empty() || preview.trim().is_empty())
                && let Ok(thread) = store.load_thread(&id)
            {
                if preview.trim().is_empty() {
                    preview = thread_preview_for_drawer(&thread);
                }
                if search_corpus.trim().is_empty() {
                    search_corpus = thread_search_corpus_for_drawer(&thread);
                }
            }
            preview = normalize_preview(&preview, 120);
            let path = store
                .thread_file_path(&id)
                .unwrap_or_else(|_| PathBuf::from(format!("thread_{id}.json")));
            let timestamp = system_time_from_unix_millis(summary.updated_at.timestamp_millis());
            let mode = transcription_mode_from_thread_mode(&summary.mode);
            let mode_label = mode_label(mode);
            if search_corpus.trim().is_empty() {
                search_corpus = format!(
                    "{} {} {} {}",
                    summary.title,
                    summary.mode,
                    summary.summary.as_deref().unwrap_or_default(),
                    preview
                );
            }
            search_corpus = format!(
                "threadstore source:thread {} thread:{} {}",
                mode_label, id, search_corpus
            )
            .to_ascii_lowercase();

            DrawerEntry {
                source,
                path,
                timestamp,
                mode,
                preview,
                search_corpus,
                is_ai_formatted: true,
                is_favorite: summary.is_favorite || favorites.contains(&favorite_key),
            }
        })
        .collect()
}

pub fn system_time_from_unix_millis(timestamp_millis: i64) -> SystemTime {
    if timestamp_millis <= 0 {
        return SystemTime::now();
    }
    UNIX_EPOCH + Duration::from_millis(timestamp_millis as u64)
}

pub fn transcription_mode_from_thread_mode(mode: &str) -> TranscriptionMode {
    if mode.eq_ignore_ascii_case("conversation") || mode.eq_ignore_ascii_case("moshi") {
        TranscriptionMode::Conversation
    } else if mode.eq_ignore_ascii_case("assistive") || mode.eq_ignore_ascii_case("chat") {
        TranscriptionMode::Assistive
    } else if mode.eq_ignore_ascii_case("hold") || mode.eq_ignore_ascii_case("raw") {
        TranscriptionMode::Hold
    } else {
        TranscriptionMode::Toggle
    }
}

pub fn normalize_preview(text: &str, max_chars: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect::<String>()
}

pub fn thread_preview_for_drawer(thread: &Thread) -> String {
    if let Some(summary) = &thread.summary
        && !summary.trim().is_empty()
    {
        return normalize_preview(summary, 120);
    }
    if let Some(note) = thread
        .notes
        .iter()
        .rev()
        .find(|note| !note.text.trim().is_empty())
    {
        return normalize_preview(&note.text, 120);
    }
    for message in thread.messages.iter().rev() {
        let text = thread_message_text_for_copy(message);
        if !text.trim().is_empty() {
            return normalize_preview(&text, 120);
        }
    }
    normalize_preview(&thread.title, 120)
}

pub fn thread_search_corpus_for_drawer(thread: &Thread) -> String {
    let mut pieces = vec![thread.title.clone(), thread.mode.clone()];
    if let Some(summary) = &thread.summary {
        pieces.push(summary.clone());
    }
    for note in &thread.notes {
        pieces.push(note.text.clone());
    }
    for message in &thread.messages {
        pieces.push(thread_message_text_for_copy(message));
    }
    pieces
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub fn drawer_entry_favorite_key(entry: &DrawerEntry) -> String {
    match &entry.source {
        DrawerEntrySource::Thread { id } => format!("thread:{id}"),
        DrawerEntrySource::LegacyFile => entry.path.to_string_lossy().to_string(),
    }
}

pub fn thread_markdown_for_copy(thread: &Thread) -> String {
    let mut out = String::new();
    let title = thread.title.trim();
    let title = if title.is_empty() {
        "Untitled Thread"
    } else {
        title
    };
    out.push_str("# ");
    out.push_str(title);
    out.push_str("\n\n");

    if let Some(summary) = &thread.summary
        && !summary.trim().is_empty()
    {
        out.push_str("## Summary\n");
        out.push_str(summary.trim());
        out.push_str("\n\n");
    }

    if !thread.notes.is_empty() {
        out.push_str("## Notes\n");
        for note in &thread.notes {
            out.push_str("- ");
            out.push_str(note.text.trim());
            if let Some(anchor) = note.anchored_to_message {
                out.push_str(&format!(" (anchor: #{anchor})"));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    if !thread.messages.is_empty() {
        out.push_str("## Messages\n");
        for message in &thread.messages {
            out.push_str("### ");
            out.push_str(&message.role.to_ascii_uppercase());
            out.push('\n');
            out.push_str(thread_message_text_for_copy(message).trim());
            out.push_str("\n\n");
        }
    }

    out.trim_end().to_string()
}

pub fn thread_messages_for_restore(thread: &Thread) -> Vec<ChatMessage> {
    let mode = mode_label(transcription_mode_from_thread_mode(&thread.mode)).to_string();
    thread
        .messages
        .iter()
        .filter_map(|message| {
            let text = thread_message_text_for_restore(message);
            if text.trim().is_empty() {
                return None;
            }
            Some(ChatMessage {
                role: chat_role_from_thread_role(&message.role),
                text,
                is_streaming: false,
                is_collapsed: false,
                is_error: false,
                timestamp: system_time_from_unix_millis(message.timestamp.timestamp_millis()),
                mode: Some(mode.clone()),
            })
        })
        .collect()
}

pub fn chat_role_from_thread_role(role: &str) -> ChatRole {
    match role.to_ascii_lowercase().as_str() {
        "assistant" => ChatRole::Assistant,
        "system" => ChatRole::System,
        _ => ChatRole::User,
    }
}

pub fn thread_message_text_for_restore(message: &codescribe_core::agent::ThreadMessage) -> String {
    let mut chunks = Vec::new();
    for value in &message.content {
        collect_restore_text(value, &mut chunks);
    }
    chunks.join(" ")
}

pub fn collect_restore_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        serde_json::Value::Array(items) => {
            if items.iter().all(serde_json::Value::is_number) {
                return;
            }
            for item in items {
                collect_restore_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(serde_json::Value::as_str)
                && !text.trim().is_empty()
            {
                out.push(text.to_string());
            }
            if let Some(content) = map.get("content") {
                collect_restore_text(content, out);
            }
            if let Some(input) = map.get("input") {
                collect_restore_text(input, out);
            }
        }
        _ => {}
    }
}

pub fn thread_message_text_for_copy(message: &codescribe_core::agent::ThreadMessage) -> String {
    let mut chunks = Vec::new();
    for value in &message.content {
        collect_copy_text(value, &mut chunks);
    }
    let text = chunks.join(" ");
    if text.trim().is_empty() {
        "(non-text content)".to_string()
    } else {
        text
    }
}

pub fn collect_copy_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        serde_json::Value::Array(items) => {
            if items.iter().all(serde_json::Value::is_number) {
                return;
            }
            for item in items {
                collect_copy_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(serde_json::Value::as_str)
                && !text.trim().is_empty()
            {
                out.push(text.to_string());
            }
            if let Some(content) = map.get("content") {
                collect_copy_text(content, out);
            }
            if let Some(input) = map.get("input") {
                collect_copy_text(input, out);
            }
            for (key, nested) in map {
                if matches!(key.as_str(), "text" | "content" | "input" | "data") {
                    continue;
                }
                collect_copy_text(nested, out);
            }
        }
        _ => {}
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct FavoritesFile {
    version: u32,
    paths: Vec<String>,
}

pub fn favorites_path() -> std::path::PathBuf {
    let dir = codescribe_core::config::Config::config_dir();
    dir.join("voice_chat_favorites.json")
}

pub fn load_favorites_from_disk() -> HashSet<String> {
    let path = favorites_path();
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let Ok(file) = serde_json::from_str::<FavoritesFile>(&data) else {
        return HashSet::new();
    };
    file.paths.into_iter().collect()
}

pub fn save_favorites_to_disk(favorites: &HashSet<String>) {
    let path = favorites_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = FavoritesFile {
        version: 1,
        paths: favorites.iter().cloned().collect(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(&path, json);
    }
}

pub fn update_drawer_after_save(path: &std::path::Path) {
    info!("Drawer entry saved: {}", path.display());
    refresh_drawer();
}
