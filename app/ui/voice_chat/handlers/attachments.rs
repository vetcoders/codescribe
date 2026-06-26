//! Attachment intake for the agent input bar.
//!
//! Drag & drop onto the overlay, file picker, clipboard paste-as-attachment
//! (file URLs and standalone images), oversized-file policy and the
//! attachment chip context-menu actions.

use super::*;

const NS_DRAG_OP_COPY: u64 = 1;
const MAX_ATTACHMENT_BYTES_UI: u64 = 50 * 1024 * 1024;
fn extract_paths_from_pasteboard(pasteboard: Id) -> Vec<PathBuf> {
    unsafe {
        let mut out = Vec::new();
        if pasteboard.is_null() {
            return out;
        }

        // Preferred path: read file URLs.
        let ns_url = Class::get("NSURL").unwrap();
        let ns_array = Class::get("NSArray").unwrap();
        let ns_dict = Class::get("NSDictionary").unwrap();
        let ns_number = Class::get("NSNumber").unwrap();

        let classes: Id = msg_send![ns_array, arrayWithObject: ns_url];
        let key = ns_string("NSPasteboardURLReadingFileURLsOnlyKey");
        let yes: Id = msg_send![ns_number, numberWithBool: true];
        let options: Id = msg_send![ns_dict, dictionaryWithObject: yes forKey: key];
        let urls: Id = msg_send![pasteboard, readObjectsForClasses: classes options: options];
        if !urls.is_null() {
            let count: usize = msg_send![urls, count];
            for i in 0..count {
                let url: Id = msg_send![urls, objectAtIndex: i];
                if url.is_null() {
                    continue;
                }
                let ns_path: Id = msg_send![url, path];
                if ns_path.is_null() {
                    continue;
                }
                let c_str: *const i8 = msg_send![ns_path, UTF8String];
                if c_str.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(c_str)
                    .to_string_lossy()
                    .to_string();
                if !s.is_empty() {
                    out.push(PathBuf::from(s));
                }
            }
        }

        // Fallback: legacy filenames pasteboard type.
        if out.is_empty() {
            let filenames_type = ns_string("NSFilenamesPboardType");
            let files: Id = msg_send![pasteboard, propertyListForType: filenames_type];
            if !files.is_null() {
                let count: usize = msg_send![files, count];
                for i in 0..count {
                    let ns_path: Id = msg_send![files, objectAtIndex: i];
                    if ns_path.is_null() {
                        continue;
                    }
                    let c_str: *const i8 = msg_send![ns_path, UTF8String];
                    if c_str.is_null() {
                        continue;
                    }
                    let s = std::ffi::CStr::from_ptr(c_str)
                        .to_string_lossy()
                        .to_string();
                    if !s.is_empty() {
                        out.push(PathBuf::from(s));
                    }
                }
            }
        }

        // Fallback for drag sources that provide raw file-url strings per pasteboard item.
        if out.is_empty() {
            let items: Id = msg_send![pasteboard, pasteboardItems];
            if !items.is_null() {
                let count: usize = msg_send![items, count];
                let file_url_type = ns_string("public.file-url");
                for i in 0..count {
                    let item: Id = msg_send![items, objectAtIndex: i];
                    if item.is_null() {
                        continue;
                    }
                    let url_str: Id = msg_send![item, stringForType: file_url_type];
                    if url_str.is_null() {
                        continue;
                    }
                    let url: Id = msg_send![ns_url, URLWithString: url_str];
                    if url.is_null() {
                        continue;
                    }
                    let is_file: bool = msg_send![url, isFileURL];
                    if !is_file {
                        continue;
                    }
                    let ns_path: Id = msg_send![url, path];
                    if ns_path.is_null() {
                        continue;
                    }
                    let c_str: *const i8 = msg_send![ns_path, UTF8String];
                    if c_str.is_null() {
                        continue;
                    }
                    let s = std::ffi::CStr::from_ptr(c_str)
                        .to_string_lossy()
                        .to_string();
                    if !s.is_empty() {
                        out.push(PathBuf::from(s));
                    }
                }
            }
        }

        out
    }
}

fn format_attachment_size_mb(size_bytes: u64) -> String {
    format!("{:.1} MB", size_bytes as f64 / (1024.0 * 1024.0))
}

pub fn show_oversized_attachments_alert(skipped: &[String]) {
    if skipped.is_empty() {
        return;
    }
    let shown = skipped
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let extra = skipped.len().saturating_sub(3);
    let suffix = if extra > 0 {
        format!(" (+{} more)", extra)
    } else {
        String::new()
    };
    let msg = format!(
        "Max attachment size is 50 MB.\nSkipped: {}{}",
        shown, suffix
    );
    show_error_alert("Attachment Too Large", &msg);
}

pub fn push_attachment_if_allowed(
    state: &mut VoiceChatOverlayState,
    attachment: Attachment,
    skipped: &mut Vec<String>,
) {
    if attachment.is_oversized() {
        skipped.push(format!(
            "{} ({}, max {} MB)",
            attachment.display_name,
            format_attachment_size_mb(attachment.size_bytes),
            MAX_ATTACHMENT_BYTES_UI / (1024 * 1024)
        ));
        return;
    }
    if !state
        .attachments
        .iter()
        .any(|a| a.same_path(&attachment.path))
    {
        state.attachments.push(attachment);
    }
}

pub extern "C" fn on_dragging_entered(_this: &Object, _cmd: Sel, dragging_info: Id) -> u64 {
    unsafe {
        let pasteboard: Id = msg_send![dragging_info, draggingPasteboard];
        let paths = extract_paths_from_pasteboard(pasteboard);
        if paths.is_empty() { 0 } else { NS_DRAG_OP_COPY }
    }
}

pub extern "C" fn on_perform_drag_operation(_this: &Object, _cmd: Sel, dragging_info: Id) -> bool {
    unsafe {
        let pasteboard: Id = msg_send![dragging_info, draggingPasteboard];
        let paths = extract_paths_from_pasteboard(pasteboard);
        if paths.is_empty() {
            return false;
        }
        let (btn_ptr, count, names, skipped) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let mut skipped = Vec::new();
            for p in paths {
                let attachment = Attachment::from_path(p, AttachmentSource::DragDrop);
                push_attachment_if_allowed(&mut state, attachment, &mut skipped);
            }
            state.attachments_last_sent = None;
            render_attachment_chips(&mut state);
            let names: Vec<String> = state
                .attachments
                .iter()
                .map(|a| a.display_name.clone())
                .collect();
            (
                state.agent_attach_button,
                state.attachments.len(),
                names,
                skipped,
            )
        };
        update_attach_button_ui(btn_ptr, count, names);
        show_oversized_attachments_alert(&skipped);
        true
    }
}
pub extern "C" fn on_attach_pick(_this: &Object, _cmd: Sel, _sender: Id) {
    let picked = crate::ui_helpers::pick_files_open_panel("Attach files");
    if picked.is_empty() {
        return;
    }

    let (btn_ptr, count, names, skipped) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut skipped = Vec::new();
        for p in picked {
            let attachment = Attachment::from_path(p, AttachmentSource::FilePicker);
            push_attachment_if_allowed(&mut state, attachment, &mut skipped);
        }
        state.attachments_last_sent = None;
        render_attachment_chips(&mut state);
        let names: Vec<String> = state
            .attachments
            .iter()
            .map(|a| a.display_name.clone())
            .collect();
        (
            state.agent_attach_button,
            state.attachments.len(),
            names,
            skipped,
        )
    };
    update_attach_button_ui(btn_ptr, count, names);
    show_oversized_attachments_alert(&skipped);
}

pub extern "C" fn on_attach_clear(_this: &Object, _cmd: Sel, _sender: Id) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.clear();
        state.attachments_last_sent = None;
        render_attachment_chips(&mut state);
        (state.agent_attach_button, 0, Vec::new())
    };
    update_attach_button_ui(btn_ptr, count, names);
}

/// Restore the attachments cleared by the previous send so the user can resend
/// them without re-picking each file. Duplicates (same path) are skipped.
pub extern "C" fn on_attach_reattach(_this: &Object, _cmd: Sel, _sender: Id) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.last_sent_attachments.is_empty() {
            return;
        }
        for attachment in state.last_sent_attachments.clone() {
            if !state.attachments.iter().any(|a| a.path == attachment.path) {
                state.attachments.push(attachment);
            }
        }
        state.attachments_last_sent = None;
        render_attachment_chips(&mut state);
        let names: Vec<String> = state
            .attachments
            .iter()
            .map(|a| a.display_name.clone())
            .collect();
        (state.agent_attach_button, state.attachments.len(), names)
    };
    update_attach_button_ui(btn_ptr, count, names);
}
// ═══════════════════════════════════════════════════════════
// Attachment Chip Handlers
// ═══════════════════════════════════════════════════════════

/// Chip body click → show context menu with Preview / Remove / Reveal / Copy Path.
pub extern "C" fn on_chip_click(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;

    unsafe {
        let ns_menu = Class::get("NSMenu").unwrap();
        let ns_menu_item = Class::get("NSMenuItem").unwrap();
        let menu: Id = msg_send![ns_menu, new];
        let handler = {
            let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.action_handler
        };
        let Some(handler_ptr) = handler else { return };
        let target = handler_ptr as Id;

        let preview: Id = msg_send![ns_menu_item, alloc];
        let preview: Id = msg_send![
            preview,
            initWithTitle: ns_string("Preview (QuickLook)")
            action: sel!(onChipPreview:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![preview, setTarget: target];
        let _: () = msg_send![preview, setTag: index as isize];
        let _: () = msg_send![menu, addItem: preview];

        let remove: Id = msg_send![ns_menu_item, alloc];
        let remove: Id = msg_send![
            remove,
            initWithTitle: ns_string("Remove")
            action: sel!(onChipRemove:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![remove, setTarget: target];
        let _: () = msg_send![remove, setTag: index as isize];
        let _: () = msg_send![menu, addItem: remove];

        let sep: Id = msg_send![ns_menu_item, separatorItem];
        let _: () = msg_send![menu, addItem: sep];

        let reveal: Id = msg_send![ns_menu_item, alloc];
        let reveal: Id = msg_send![
            reveal,
            initWithTitle: ns_string("Reveal in Finder")
            action: sel!(onChipReveal:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![reveal, setTarget: target];
        let _: () = msg_send![reveal, setTag: index as isize];
        let _: () = msg_send![menu, addItem: reveal];

        let copy_path: Id = msg_send![ns_menu_item, alloc];
        let copy_path: Id = msg_send![
            copy_path,
            initWithTitle: ns_string("Copy Path")
            action: sel!(onChipCopyPath:)
            keyEquivalent: ns_string("")
        ];
        let _: () = msg_send![copy_path, setTarget: target];
        let _: () = msg_send![copy_path, setTag: index as isize];
        let _: () = msg_send![menu, addItem: copy_path];

        // Pop up at mouse location.
        let ns_event = Class::get("NSEvent").unwrap();
        let location: CGPoint = msg_send![ns_event, mouseLocation];
        let _: bool = msg_send![
            menu,
            popUpMenuPositioningItem: std::ptr::null_mut::<Object>()
            atLocation: location
            inView: std::ptr::null_mut::<Object>()
        ];
    }
}

pub extern "C" fn on_chip_remove(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    crate::ui::voice_chat::api::remove_attachment_at(index);
}

pub extern "C" fn on_chip_preview(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.get(index).map(|a| a.path.clone())
    };
    if let Some(path) = path {
        // Use macOS QuickLook for native preview.
        std::thread::spawn(move || {
            let _ = std::process::Command::new("qlmanage")
                .arg("-p")
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
    }
}

pub extern "C" fn on_chip_reveal(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.attachments.get(index).map(|a| a.path.clone())
    };
    if let Some(path) = path {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn();
    }
}

pub extern "C" fn on_chip_copy_path(_this: &Object, _cmd: Sel, sender: Id) {
    let index: isize = unsafe { msg_send![sender, tag] };
    let index = index.max(0) as usize;
    let path = {
        let state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .attachments
            .get(index)
            .map(|a| a.path.display().to_string())
    };
    if let Some(path) = path {
        copy_to_clipboard(&path);
    }
}
// ═══════════════════════════════════════════════════════════
// Paste-as-attachment
// ═══════════════════════════════════════════════════════════

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PasteDisposition {
    Attachment,
    TextPaste,
}

pub(super) fn paste_disposition(
    has_files: bool,
    has_image: bool,
    has_text: bool,
) -> PasteDisposition {
    if has_files || (has_image && !has_text) {
        PasteDisposition::Attachment
    } else {
        PasteDisposition::TextPaste
    }
}

/// Check the general pasteboard and, if it contains file URLs or a standalone image
/// (no accompanying text), treat the paste as an attachment instead of text insertion.
///
/// Returns `true` when the paste was consumed as an attachment (caller should suppress
/// the default NSTextView paste), or `false` to let the default paste proceed.
///
/// # Safety
/// Must be called on the main thread. Uses Objective-C messaging.
pub unsafe fn try_paste_as_attachment() -> bool {
    let ns_pasteboard = Class::get("NSPasteboard").unwrap();
    let pasteboard: Id = msg_send![ns_pasteboard, generalPasteboard];

    // 1. File URLs → always treat as attachments
    let file_paths = extract_paths_from_pasteboard(pasteboard);
    if paste_disposition(!file_paths.is_empty(), false, false) == PasteDisposition::Attachment {
        let (btn_ptr, count, names, skipped) = {
            let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let mut skipped = Vec::new();
            for p in file_paths {
                let attachment = Attachment::from_path(p, AttachmentSource::Clipboard);
                push_attachment_if_allowed(&mut state, attachment, &mut skipped);
            }
            state.attachments_last_sent = None;
            render_attachment_chips(&mut state);
            let names: Vec<String> = state
                .attachments
                .iter()
                .map(|a| a.display_name.clone())
                .collect();
            (
                state.agent_attach_button,
                state.attachments.len(),
                names,
                skipped,
            )
        };
        update_attach_button_ui(btn_ptr, count, names);
        show_oversized_attachments_alert(&skipped);
        debug!("Paste intercepted: {} file(s) attached", count);
        return true;
    }

    // 2. Check for image data WITHOUT accompanying text
    let has_image = unsafe { pasteboard_has_type(pasteboard, "public.tiff") }
        || unsafe { pasteboard_has_type(pasteboard, "public.png") };
    let has_text = unsafe { pasteboard_has_type(pasteboard, "public.utf8-plain-text") };

    if paste_disposition(false, has_image, has_text) == PasteDisposition::Attachment {
        // Read PNG data from pasteboard (try PNG first, then TIFF→PNG conversion)
        if let Some(image_data) = unsafe { read_image_from_pasteboard(pasteboard) } {
            match AttachmentStore::save_clipboard_image(&image_data, "png") {
                Ok(path) => {
                    let (btn_ptr, count, names, skipped) = {
                        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                        let mut skipped = Vec::new();
                        let attachment = Attachment::from_path(path, AttachmentSource::Clipboard);
                        push_attachment_if_allowed(&mut state, attachment, &mut skipped);
                        state.attachments_last_sent = None;
                        render_attachment_chips(&mut state);
                        let names: Vec<String> = state
                            .attachments
                            .iter()
                            .map(|a| a.display_name.clone())
                            .collect();
                        (
                            state.agent_attach_button,
                            state.attachments.len(),
                            names,
                            skipped,
                        )
                    };
                    update_attach_button_ui(btn_ptr, count, names);
                    show_oversized_attachments_alert(&skipped);
                    debug!("Paste intercepted: clipboard image saved as attachment");
                    return true;
                }
                Err(e) => {
                    info!("Failed to save clipboard image: {e}");
                    // Fall through to default paste
                }
            }
        }
    }

    // 3. Text paste (with or without image) → default behaviour
    false
}

/// Check if the pasteboard contains a given UTI type.
///
/// # Safety
/// Requires main thread and valid pasteboard pointer.
unsafe fn pasteboard_has_type(pasteboard: Id, uti: &str) -> bool {
    let ns_array = Class::get("NSArray").unwrap();
    let type_str = ns_string(uti);
    let types: Id = msg_send![ns_array, arrayWithObject: type_str];
    let available: Id = msg_send![pasteboard, availableTypeFromArray: types];
    !available.is_null()
}

/// Read image data from the pasteboard as PNG bytes.
///
/// Tries `public.png` first. Falls back to `public.tiff` and converts to PNG
/// via `NSBitmapImageRep`.
///
/// # Safety
/// Requires main thread and valid pasteboard pointer.
unsafe fn read_image_from_pasteboard(pasteboard: Id) -> Option<Vec<u8>> {
    // Try PNG first
    let png_type = ns_string("public.png");
    let png_data: Id = msg_send![pasteboard, dataForType: png_type];
    if !png_data.is_null() {
        let length: usize = msg_send![png_data, length];
        let bytes: *const u8 = msg_send![png_data, bytes];
        if !bytes.is_null() && length > 0 {
            return Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec());
        }
    }

    // Fallback: TIFF → convert to PNG via NSBitmapImageRep
    let tiff_type = ns_string("public.tiff");
    let tiff_data: Id = msg_send![pasteboard, dataForType: tiff_type];
    if tiff_data.is_null() {
        return None;
    }

    let ns_bitmap = Class::get("NSBitmapImageRep")?;
    let rep: Id = msg_send![ns_bitmap, imageRepWithData: tiff_data];
    if rep.is_null() {
        return None;
    }

    // NSBitmapImageFileType.png = 4
    let png_file_type: usize = 4;
    let nil: Id = std::ptr::null_mut();
    let result: Id = msg_send![
        rep,
        representationUsingType: png_file_type
        properties: nil
    ];
    if result.is_null() {
        return None;
    }

    let length: usize = msg_send![result, length];
    let bytes: *const u8 = msg_send![result, bytes];
    if bytes.is_null() || length == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())
}
