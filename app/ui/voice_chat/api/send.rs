//! Draft sending, send callback wiring, attachments and input sizing.

use super::*;

/// Dispatch a payload through the registered chat send callback without mutating bubbles.
///
/// Returns `true` when a callback was found and invoked.
pub fn dispatch_voice_chat_send(payload: &str) -> bool {
    let payload = payload.trim();
    if payload.is_empty() {
        return false;
    }
    let handler = {
        let guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(handler) = handler {
        handler(payload.to_string());
        true
    } else {
        warn!("No voice-chat send callback set; cannot dispatch runtime send request");
        false
    }
}

/// Submit the current draft (manual send)
pub fn send_voice_chat_draft() {
    Queue::main().exec_async(|| {
        send_draft_message_impl();
    });
}

/// Set the send callback invoked when the user submits a message
pub fn set_voice_chat_send_callback(
    callback: Option<crate::ui::voice_chat::state::VoiceChatSendCallback>,
) {
    let mut handler = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *handler = callback.clone();
    drop(handler);

    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.auto_send_enabled = callback.is_some();
    update_send_button_with_state(&mut state);
}

/// Toggle loading state for sending
pub fn set_voice_chat_sending(is_sending: bool) {
    Queue::main().exec_async(move || {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.is_sending = is_sending;
        update_send_button_with_state(&mut state);
    });
}

/// Clear all chat messages and reset input state
pub fn clear_voice_chat_text() {
    Queue::main().exec_async(|| {
        clear_voice_chat_text_impl();
    });
}

/// Start a fresh Agent thread by rotating backend runtime first, then clearing UI state.
pub fn start_new_thread_impl() {
    update_voice_chat_status_impl("Starting new thread...");

    std::thread::spawn(|| {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(error) => {
                let reason = format!("Unable to initialize async runtime for New thread: {error}");
                Queue::main().exec_async(move || {
                    warn!("{reason}");
                    update_voice_chat_status_impl("Thread reset failed");
                    add_voice_chat_error_message(&reason);
                });
                return;
            }
        };

        let reset_result = rt.block_on(crate::controller::reset_agent_runtime_for_new_thread());
        Queue::main().exec_async(move || match reset_result {
            Ok(generation) => {
                clear_voice_chat_text_impl();
                update_voice_chat_status_impl("Ready");
                info!("New thread started (generation={generation})");
            }
            Err(error) => {
                warn!("Failed to start new thread: {error}");
                update_voice_chat_status_impl("Thread reset failed");
                add_voice_chat_error_message(&format!(
                    "Unable to start a new thread. Continuing the current thread. {error}"
                ));
            }
        });
    });
}

pub fn clear_voice_chat_text_impl() {
    let btn_ptr = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.messages.clear();
        state.active_user_stream_index = None;
        state.active_assistant_stream_index = None;
        state.active_reasoning_stream_index = None;
        state.manual_draft.clear();
        state.is_sending = false;
        state.attachments.clear();
        state.attachments_last_sent = None;
        render_attachment_chips_locked(&mut state);
        let btn_ptr = state.agent_attach_button;

        if let Some(input_view) = state.agent_input_text_view {
            unsafe { set_text_view_string(input_view as Id, "") };
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { set_text_field_string(input_field as Id, "") };
        }
        resize_agent_input_locked(&mut state);

        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        btn_ptr
    };
    update_attach_button_ui(btn_ptr, 0, Vec::new());
}

/// Send the draft message (called from handlers)
pub fn send_draft_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let draft = if let Some(text_view) = state.agent_input_text_view {
            unsafe { get_text_view_string(text_view as Id) }
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { get_text_field_string(input_field as Id) }
        } else {
            return;
        };
        let draft = draft.trim().to_string();
        if draft.is_empty() {
            return;
        }

        // Check handler BEFORE mutating state to avoid phantom messages
        // when no connector is registered.
        let handler_guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(handler) = handler_guard.clone() else {
            // No send handler — leave state untouched so draft remains in input.
            return;
        };
        drop(handler_guard);

        let attachments_to_send = attachment_should_include_locked(&state);
        // Commit fingerprint under same lock to prevent race with concurrent attachment changes.
        if let Some((fp, _, _)) = attachments_to_send.as_ref() {
            state.attachments_last_sent = Some(*fp);
        }
        if let Some((_fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_collapsed: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
        }

        let mode = message_mode_label(&state);
        state.messages.push(ChatMessage {
            role: ChatRole::User,
            text: draft.clone(),
            is_streaming: false,
            is_collapsed: false,
            is_error: false,
            timestamp: SystemTime::now(),
            mode: Some(mode),
        });
        state.manual_draft.clear();
        state.is_sending = true;
        if let Some(text_view) = state.agent_input_text_view {
            unsafe { set_text_view_string(text_view as Id, "") };
        } else if let Some(input_field) = state.agent_input_field {
            unsafe { set_text_field_string(input_field as Id, "") };
        }
        resize_agent_input_locked(&mut state);
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        (handler, draft, attachments_to_send)
    };

    let (handler, draft, attachments_to_send) = callback;
    if let Some((_fingerprint, paths, _summary)) = attachments_to_send {
        std::thread::spawn(move || {
            let block = build_attachments_block(&paths);
            let payload = if block.is_empty() {
                draft
            } else {
                format!("{draft}\n\n{block}")
            };
            // The send callback uses `tokio::spawn`, which requires a runtime handle.
            // Calling it from an arbitrary background thread can panic (release builds abort).
            Queue::main().exec_async(move || handler(payload));
        });
    } else {
        handler(draft);
    }
}

pub fn commit_last_user_message_impl() {
    let callback = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let Some(last_message) = state.messages.last() else {
            return;
        };
        if last_message.role != ChatRole::User {
            return;
        }
        let text = last_message.text.clone();

        // Check handler BEFORE mutating state to avoid phantom messages.
        let handler_guard = SEND_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(handler) = handler_guard.clone() else {
            return;
        };
        drop(handler_guard);

        let attachments_to_send = attachment_should_include_locked(&state);
        // Commit fingerprint under same lock to prevent race with concurrent attachment changes.
        if let Some((fp, _, _)) = attachments_to_send.as_ref() {
            state.attachments_last_sent = Some(*fp);
        }
        if let Some((_fingerprint, _paths, summary)) = attachments_to_send.as_ref() {
            let mode = message_mode_label(&state);
            state.messages.push(ChatMessage {
                role: ChatRole::System,
                text: format!("Attachments (sent once): {}", summary),
                is_streaming: false,
                is_collapsed: false,
                is_error: false,
                timestamp: SystemTime::now(),
                mode: Some(mode),
            });
        }
        state.is_sending = true;
        update_chat_view_with_state(&mut state, true);
        update_send_button_with_state(&mut state);
        (handler, text, attachments_to_send)
    };

    let (handler, text, attachments_to_send) = callback;
    if let Some((_fingerprint, paths, _summary)) = attachments_to_send {
        std::thread::spawn(move || {
            let block = build_attachments_block(&paths);
            let payload = if block.is_empty() {
                text
            } else {
                format!("{text}\n\n{block}")
            };
            Queue::main().exec_async(move || handler(payload));
        });
    } else {
        handler(text);
    }
}

pub fn discard_last_message_impl() {
    let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.messages.pop().is_some() {
        if let Some(idx) = state.active_user_stream_index
            && idx >= state.messages.len()
        {
            state.active_user_stream_index = None;
        }
        if let Some(idx) = state.active_assistant_stream_index
            && idx >= state.messages.len()
        {
            state.active_assistant_stream_index = None;
        }
        if let Some(idx) = state.active_reasoning_stream_index
            && idx >= state.messages.len()
        {
            state.active_reasoning_stream_index = None;
        }
        update_chat_view_with_state(&mut state, true);
    }
}

pub fn update_send_button_with_state(state: &mut VoiceChatOverlayState) {
    unsafe {
        if let Some(button_ptr) = state.agent_send_button {
            let btn = button_ptr as Id;
            let enabled = !state.is_sending && state.auto_send_enabled;
            let _: () = msg_send![btn, setEnabled: enabled];
            let symbol = if state.is_sending {
                "ellipsis.circle"
            } else {
                "arrow.up.circle.fill"
            };
            let has_symbol = crate::ui_helpers::set_button_symbol(btn, symbol);
            let title = if has_symbol {
                ""
            } else if state.is_sending {
                "…"
            } else {
                "Send"
            };
            let _: () = msg_send![btn, setTitle: ns_string(title)];
        }
    }
}

pub fn update_attach_button_ui(btn_ptr: Option<usize>, count: usize, mut names: Vec<String>) {
    unsafe {
        let Some(btn_ptr) = btn_ptr else {
            return;
        };
        let btn = btn_ptr as Id;
        let has_symbol = crate::ui_helpers::set_button_symbol(btn, "paperclip");
        let title = if count == 0 {
            if has_symbol {
                String::new()
            } else {
                "Attach".to_string()
            }
        } else if has_symbol {
            String::new()
        } else {
            count.to_string()
        };
        let _: () = msg_send![btn, setTitle: ns_string(&title)];

        if count == 0 {
            crate::ui_helpers::set_tooltip(btn, "Attach files (assistant context)");
        } else {
            names.sort();
            let shown: Vec<String> = names.into_iter().take(3).collect();
            let suffix = if count > 3 { "…" } else { "" };
            let tip = format!("Attached: {}{}", shown.join(", "), suffix);
            let _: () = msg_send![btn, setToolTip: ns_string(&tip)];
        }
    }
}

pub fn attachment_should_include_locked(
    state: &VoiceChatOverlayState,
) -> Option<(u64, Vec<std::path::PathBuf>, String)> {
    if state.attachments.is_empty() {
        return None;
    }
    let fingerprint = attachment_fingerprint(&state.attachments);
    if state.attachments_last_sent == Some(fingerprint) {
        return None;
    }
    let summary = attachment_summary(&state.attachments);
    let paths = Attachment::paths(&state.attachments);
    Some((fingerprint, paths, summary))
}

pub fn attachment_summary(attachments: &[Attachment]) -> String {
    let mut names: Vec<String> = attachments.iter().map(|a| a.display_name.clone()).collect();
    names.sort();
    if names.len() <= 3 {
        names.join(", ")
    } else {
        format!("{}, … (+{})", names[..3].join(", "), names.len() - 3)
    }
}

pub fn attachment_fingerprint(attachments: &[Attachment]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for a in attachments {
        a.path.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(&a.path) {
            meta.len().hash(&mut hasher);
            meta.modified().ok().hash(&mut hasher);
        }
    }
    hasher.finish()
}

// ═══════════════════════════════════════════════════════════
// Attachment Chip Strip
// ═══════════════════════════════════════════════════════════

pub const CHIP_STRIP_HEIGHT: f64 = 36.0;

/// Remove an attachment by index, re-render chips, update button.
pub fn remove_attachment_at(index: usize) {
    let (btn_ptr, count, names) = {
        let mut state = OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if index < state.attachments.len() {
            state.attachments.remove(index);
            state.attachments_last_sent = None;
        }
        let names: Vec<String> = state
            .attachments
            .iter()
            .map(|a| a.display_name.clone())
            .collect();
        render_attachment_chips_locked(&mut state);
        (state.agent_attach_button, state.attachments.len(), names)
    };
    update_attach_button_ui(btn_ptr, count, names);
}

/// Rebuild chip strip views from current attachments.
/// Must be called from the main thread.
pub fn render_attachment_chips(state: &mut VoiceChatOverlayState) {
    render_attachment_chips_locked(state);
}

pub fn render_attachment_chips_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let Some(strip_ptr) = state.attachment_chip_strip else {
            return;
        };
        let strip = strip_ptr as Id;

        // Get the stack view (document view of the scroll view).
        let stack: Id = msg_send![strip, documentView];
        if stack.is_null() {
            return;
        }

        // Clear existing chips.
        let arranged: Id = msg_send![stack, arrangedSubviews];
        let old_count: usize = msg_send![arranged, count];
        for i in (0..old_count).rev() {
            let view: Id = msg_send![arranged, objectAtIndex: i];
            let _: () = msg_send![stack, removeArrangedSubview: view];
            let _: () = msg_send![view, removeFromSuperview];
        }

        let has_attachments = !state.attachments.is_empty();
        let handler_ptr = match state.action_handler {
            Some(p) => p as Id,
            None => std::ptr::null_mut::<Object>(),
        };

        if has_attachments {
            let mut total_width = 0.0f64;
            for (idx, attachment) in state.attachments.iter().enumerate() {
                let chip = create_chip_view(idx, &attachment.chip_label(20), handler_ptr);
                let _: () = msg_send![stack, addArrangedSubview: chip];
                let chip_frame: CGRect = msg_send![chip, frame];
                total_width += chip_frame.size.width + 6.0;
            }
            // Size the stack view to fit all chips (enables horizontal scrolling).
            let strip_frame: CGRect = msg_send![strip, frame];
            let stack_frame = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(total_width.max(strip_frame.size.width), CHIP_STRIP_HEIGHT),
            );
            let _: () = msg_send![stack, setFrame: stack_frame];
        }

        // Show/hide the strip.
        let currently_hidden: bool = msg_send![strip, isHidden];
        if currently_hidden == has_attachments {
            let _: () = msg_send![strip, setHidden: !has_attachments];
        }
    }
    // Reflow layout to account for chip strip height change.
    resize_agent_input_locked(state);
}

/// Create a single chip view: a styled button with the attachment name.
///
/// # Safety
/// Requires main thread.
pub unsafe fn create_chip_view(index: usize, label: &str, handler: Id) -> Id {
    let ns_button = Class::get("NSButton").unwrap();

    // Measure text width (approximate: 7px per char + padding).
    let text_width = (label.chars().count() as f64 * 7.0).clamp(40.0, 180.0);
    let chip_width = text_width + 24.0; // padding

    let frame = CGRect::new(&CGPoint::new(0.0, 4.0), &CGSize::new(chip_width, 28.0));
    let btn: Id = msg_send![ns_button, alloc];
    let btn: Id = msg_send![btn, initWithFrame: frame];
    let _: () = msg_send![btn, setTitle: ns_string(label)];
    // NSBezelStyleInline = 15 (compact rounded)
    let _: () = msg_send![btn, setBezelStyle: 15i64];
    let _: () = msg_send![btn, setControlSize: 1i64]; // NSControlSizeSmall
    let ns_font = Class::get("NSFont").unwrap();
    let font: Id = msg_send![ns_font, systemFontOfSize: 11.0f64];
    let _: () = msg_send![btn, setFont: font];
    let _: () = msg_send![btn, setTag: index as isize];
    if !handler.is_null() {
        let _: () = msg_send![btn, setTarget: handler];
        let _: () = msg_send![btn, setAction: sel!(onChipClick:)];
    }
    let _: () = msg_send![btn, setTranslatesAutoresizingMaskIntoConstraints: false];

    // Height constraint.
    let height_anchor: Id = msg_send![btn, heightAnchor];
    let constraint: Id = msg_send![height_anchor, constraintEqualToConstant: 28.0f64];
    let _: () = msg_send![constraint, setActive: true];

    btn
}

pub fn build_attachments_block(paths: &[std::path::PathBuf]) -> String {
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const MAX_TOTAL_CHARS: usize = 120_000;
    const MAX_FILE_CHARS: usize = 40_000;
    const MAX_FILE_BYTES: usize = 512 * 1024; // cap IO; we only inline a prefix anyway
    const PDF_MIN_TEXT_CHARS: usize = 100;

    fn env_usize(key: &str, default_value: usize) -> usize {
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(default_value)
    }

    fn env_bool(key: &str, default_value: bool) -> bool {
        std::env::var(key)
            .ok()
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(default_value)
    }

    fn tool_env_key(name: &str) -> String {
        format!(
            "CODESCRIBE_TOOL_{}",
            name.to_ascii_uppercase().replace('-', "_")
        )
    }

    fn tool_path(name: &str) -> Option<PathBuf> {
        if let Ok(v) = std::env::var(tool_env_key(name)) {
            let v = v.trim();
            if !v.is_empty() {
                let p = PathBuf::from(v);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        // macOS GUI apps often have a minimal PATH that doesn't include Homebrew.
        // Prefer common install locations so this works when launched from Finder.
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            let p = Path::new(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }

        None
    }

    fn tool_command(name: &str) -> Command {
        if let Some(p) = tool_path(name) {
            Command::new(p)
        } else {
            Command::new(name)
        }
    }

    fn command_exists(name: &str) -> bool {
        tool_path(name).is_some()
    }

    fn run_command_stdout(mut cmd: Command) -> Result<Vec<u8>, String> {
        let output = cmd.output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                "command failed".to_string()
            } else {
                stderr
            })
        }
    }

    fn extract_pdf_text_pdftotext(path: &std::path::Path, pages: usize) -> Result<String, String> {
        let pages = pages.max(1);
        let mut cmd = tool_command("pdftotext");
        cmd.args(["-f", "1", "-l", &pages.to_string()])
            .arg(path)
            .arg("-");
        let stdout = run_command_stdout(cmd)?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    fn temp_dir(prefix: &str) -> Result<std::path::PathBuf, String> {
        let pid = std::process::id();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let dir = std::env::temp_dir().join(format!("{prefix}_{pid}_{stamp}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    fn extract_pdf_text_ocrmypdf(
        path: &std::path::Path,
        pages: usize,
        language: &str,
    ) -> Result<String, String> {
        let pages = pages.max(1);
        let dir = temp_dir("codescribe_pdf_ocr")?;
        let output_pdf = dir.join("ocr.pdf");

        // NOTE: we OCR only first N pages to keep latency acceptable.
        // `ocrmypdf` doesn't support "first N pages" directly, so we pre-split via `pdftk`/`qpdf`
        // would add more deps; instead we run ocrmypdf on whole doc only when asked.
        // Here: best-effort; if you want faster, disable OCR or raise pages and accept cost.
        //
        // We still respect `pages` when extracting with pdftotext after OCR.
        let _ = pages;
        let mut cmd = tool_command("ocrmypdf");
        cmd.args([
            "--language",
            language,
            "--force-ocr",
            "--clean",
            "--deskew",
            "--remove-background",
        ])
        .arg(path)
        .arg(&output_pdf);
        let _ = run_command_stdout(cmd)?;

        let text = extract_pdf_text_pdftotext(&output_pdf, pages).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        Ok(text)
    }

    fn extract_pdf_text_tesseract(
        path: &std::path::Path,
        pages: usize,
        language: &str,
    ) -> Result<String, String> {
        let pages = pages.max(1);
        let dir = temp_dir("codescribe_pdf_pages")?;
        let prefix = dir.join("page");

        if command_exists("pdftoppm") {
            let mut cmd = tool_command("pdftoppm");
            cmd.args(["-png", "-r", "300", "-f", "1", "-l", &pages.to_string()])
                .arg(path)
                .arg(&prefix);
            let _ = run_command_stdout(cmd)?;
        } else if command_exists("convert") {
            // ImageMagick fallback: convert first N pages (best-effort).
            let output = dir.join("page-%03d.png");
            let mut cmd = tool_command("convert");
            cmd.args(["-density", "300"])
                .arg(path)
                .args(["-quality", "100"])
                .arg(output);
            let _ = run_command_stdout(cmd)?;
        } else {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("Missing pdftoppm/convert for PDF->image".to_string());
        }

        let mut images: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
                    .unwrap_or(false)
            })
            .collect();
        images.sort();

        if images.is_empty() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("PDF->image produced no pages".to_string());
        }

        if !command_exists("tesseract") {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("Missing tesseract".to_string());
        }

        let mut out = String::new();
        for (i, img) in images.iter().take(pages).enumerate() {
            let mut cmd = tool_command("tesseract");
            cmd.arg(img).arg("stdout").args(["-l", language]);
            let stdout = run_command_stdout(cmd)?;
            let text = String::from_utf8_lossy(&stdout);
            out.push_str(&format!("=== PAGE {} ===\n", i + 1));
            out.push_str(text.trim());
            out.push_str("\n\n");
        }

        let _ = std::fs::remove_dir_all(&dir);
        Ok(out)
    }

    fn extract_pdf_text_auto(
        path: &std::path::Path,
        pages: usize,
    ) -> Result<(String, &'static str), String> {
        let text = extract_pdf_text_pdftotext(path, pages).unwrap_or_default();
        if text.trim().chars().count() >= PDF_MIN_TEXT_CHARS {
            return Ok((text, "pdftotext"));
        }

        let ocr_enabled = env_bool("CODESCRIBE_ATTACH_PDF_OCR", true);
        if !ocr_enabled {
            return Ok((text, "pdftotext (minimal text)"));
        }

        fn default_ocr_lang() -> String {
            if let Some(v) = std::env::var("CODESCRIBE_ATTACH_PDF_OCR_LANG")
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                return v;
            }

            // Prefer Polish+English when available, otherwise fall back to English.
            let mut has_pol = false;
            let mut has_eng = false;
            if command_exists("tesseract") {
                let mut cmd = tool_command("tesseract");
                cmd.arg("--list-langs");
                if let Ok(stdout) = run_command_stdout(cmd) {
                    for line in String::from_utf8_lossy(&stdout).lines() {
                        let l = line.trim();
                        if l == "pol" {
                            has_pol = true;
                        }
                        if l == "eng" {
                            has_eng = true;
                        }
                    }
                }
            }

            if has_pol && has_eng {
                "pol+eng".to_string()
            } else if has_eng {
                "eng".to_string()
            } else if has_pol {
                "pol".to_string()
            } else {
                "eng".to_string()
            }
        }

        let ocr_lang = default_ocr_lang();

        if command_exists("ocrmypdf")
            && command_exists("pdftotext")
            && let Ok(ocr_text) = extract_pdf_text_ocrmypdf(path, pages, &ocr_lang)
            && ocr_text.trim().chars().count() >= PDF_MIN_TEXT_CHARS
        {
            return Ok((ocr_text, "ocrmypdf+pdftotext"));
        }

        if let Ok(ocr_text) = extract_pdf_text_tesseract(path, pages, &ocr_lang)
            && ocr_text.trim().chars().count() >= PDF_MIN_TEXT_CHARS
        {
            return Ok((ocr_text, "tesseract"));
        }

        Ok((text, "pdftotext (minimal text)"))
    }

    fn extract_pdf_quicklook_ocr(
        path: &std::path::Path,
        language: &str,
    ) -> Result<(String, &'static str), String> {
        if !command_exists("qlmanage") {
            return Err("Missing qlmanage".to_string());
        }
        if !command_exists("tesseract") {
            return Err("Missing tesseract".to_string());
        }

        let dir = temp_dir("codescribe_pdf_ql")?;
        let mut cmd = tool_command("qlmanage");
        cmd.args(["-t", "-s", "1400", "-o"]).arg(&dir).arg(path);
        let _ = run_command_stdout(cmd)?;

        let mut images: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .collect();
        images.sort();

        let Some(img) = images.first() else {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("QuickLook produced no PNG".to_string());
        };

        let mut cmd = tool_command("tesseract");
        cmd.arg(img).arg("stdout").args(["-l", language]);
        let stdout = run_command_stdout(cmd)?;
        let text = String::from_utf8_lossy(&stdout).into_owned();
        let _ = std::fs::remove_dir_all(&dir);
        Ok((text, "quicklook+tesseract"))
    }

    let mut out = String::new();
    out.push_str("ATTACHMENTS (file context)\n");

    let mut total_chars = out.chars().count();
    let mut image_paths: Vec<String> = Vec::new();
    let pdf_pages = env_usize("CODESCRIBE_ATTACH_PDF_PAGES", 3);
    for path in paths {
        if total_chars >= MAX_TOTAL_CHARS {
            break;
        }

        let display = path.to_string_lossy();
        out.push_str("\n---\n");
        out.push_str(&format!("FILE: {display}\n"));

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "pdf" {
            let ocr_lang = std::env::var("CODESCRIBE_ATTACH_PDF_OCR_LANG")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "pol+eng".to_string());

            let extracted = if command_exists("pdftotext") {
                extract_pdf_text_auto(path, pdf_pages).ok()
            } else {
                // Offline-friendly fallback: render first page via QuickLook and OCR it.
                extract_pdf_quicklook_ocr(path, &ocr_lang).ok()
            };

            match extracted {
                Some((mut text, method)) => {
                    // Cap per-file and total.
                    if text.chars().count() > MAX_FILE_CHARS {
                        text = text.chars().take(MAX_FILE_CHARS).collect();
                        text.push_str("\n… (truncated)\n");
                    }

                    let remaining = MAX_TOTAL_CHARS.saturating_sub(total_chars);
                    if remaining == 0 {
                        break;
                    }
                    let mut snippet: String = text.chars().take(remaining).collect();
                    if snippet.len() < text.len() {
                        snippet.push_str("\n… (truncated)\n");
                    }

                    let pages_hint = if method == "quicklook+tesseract" {
                        "1".to_string()
                    } else {
                        pdf_pages.to_string()
                    };
                    out.push_str(&format!(
                        "(PDF text extracted via {method}; pages: {pages_hint})\n"
                    ));
                    out.push_str("```text\n");
                    out.push_str(&snippet);
                    if !snippet.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n");
                }
                None => {
                    out.push_str(
                        "(PDF: couldn't extract text right now. Quick fix: copy 1-2 pages as text or attach a screenshot (vision).\n\
Tools (optional): `brew install poppler ocrmypdf tesseract-lang`.)\n",
                    );
                }
            }

            total_chars = out.chars().count();
            continue;
        }

        let Ok(mut f) = std::fs::File::open(path) else {
            out.push_str("(failed to open)\n");
            continue;
        };

        let mut buf = Vec::new();
        let _ = (&mut f).take(MAX_FILE_BYTES as u64).read_to_end(&mut buf);

        let Ok(mut s) = String::from_utf8(buf) else {
            let is_image = matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff"
            );
            if is_image {
                out.push_str("(image detected; will be sent as vision input)\n");
                image_paths.push(display.to_string());
            } else {
                out.push_str("(skipped: not UTF-8 text)\n");
            }
            continue;
        };

        // Normalize + cap per-file.
        if s.chars().count() > MAX_FILE_CHARS {
            s = s.chars().take(MAX_FILE_CHARS).collect();
            s.push_str("\n… (truncated)\n");
        }

        // Cap total.
        let remaining = MAX_TOTAL_CHARS.saturating_sub(total_chars);
        if remaining == 0 {
            break;
        }
        let mut snippet: String = s.chars().take(remaining).collect();
        if snippet.len() < s.len() {
            snippet.push_str("\n… (truncated)\n");
        }

        out.push_str("```text\n");
        out.push_str(&snippet);
        if !snippet.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");

        total_chars = out.chars().count();
    }

    if !image_paths.is_empty() && total_chars < MAX_TOTAL_CHARS {
        out.push_str("\n---\n");
        out.push_str("ATTACHMENTS (image paths)\n");
        image_paths.sort();
        for p in image_paths {
            if total_chars >= MAX_TOTAL_CHARS {
                break;
            }
            out.push_str("- ");
            out.push_str(&p);
            out.push('\n');
            total_chars = out.chars().count();
        }
    }

    out
}

/// Resize the Agent input bar based on current draft text.
///
/// Keeps it compact by default, and grows it when the user types/pastes longer messages.
pub fn resize_agent_input_to_draft() {
    let Ok(mut state) = OVERLAY_STATE.try_lock() else {
        return;
    };
    resize_agent_input_locked(&mut state);
}

pub fn resize_agent_input_locked(state: &mut VoiceChatOverlayState) {
    unsafe {
        let (
            Some(window_ptr),
            Some(bar_ptr),
            Some(scroll_ptr),
            Some(text_view_ptr),
            Some(attach_ptr),
            Some(send_ptr),
        ) = (
            state.window,
            state.agent_input_bar,
            state.agent_input_scroll_view,
            state.agent_input_text_view,
            state.agent_attach_button,
            state.agent_send_button,
        )
        else {
            return;
        };

        let input_bar = bar_ptr as Id;
        let input_scroll = scroll_ptr as Id;
        let text_view = text_view_ptr as Id;
        let attach_btn = attach_ptr as Id;
        let send_btn = send_ptr as Id;

        let (content_width, content_height) =
            if let Some(container_ptr) = state.split_content_container {
                let container = container_ptr as Id;
                let _: () = msg_send![container, setNeedsLayout: true];
                let _: () = msg_send![container, layoutSubtreeIfNeeded];
                let bounds: CGRect = msg_send![container, bounds];
                (bounds.size.width, bounds.size.height)
            } else {
                let window = window_ptr as Id;
                let window_frame: CGRect = msg_send![window, frame];
                (window_frame.size.width, window_frame.size.height)
            };

        let text = get_text_view_string(text_view);

        // Keep the input compact by default (single-line-ish), then grow smoothly up to a cap.
        let min_h = 44.0;
        let max_h = 180.0;
        let desired_h = if text.trim().is_empty() {
            min_h
        } else {
            // Prefer actual layout height from NSTextView; fall back to a simple heuristic.
            let mut measured: Option<f64> = None;
            let layout: Id = msg_send![text_view, layoutManager];
            let container: Id = msg_send![text_view, textContainer];
            if !layout.is_null() && !container.is_null() {
                let _: () = msg_send![layout, ensureLayoutForTextContainer: container];
                let used: CGRect = msg_send![layout, usedRectForTextContainer: container];
                let text_h = used.size.height.max(0.0);
                measured = Some((text_h + 20.0).clamp(min_h, max_h));
            }

            measured.unwrap_or_else(|| {
                let hard_lines = (text.matches('\n').count() + 1).max(1);
                // Heuristic for wrapped lines: assume ~52 chars per visual line at this width.
                let wrapped_lines = text.chars().count().div_ceil(52).max(1);
                let visual_lines = hard_lines.max(wrapped_lines);
                let line_h = 18.0;
                (min_h + (visual_lines.saturating_sub(1) as f64) * line_h).clamp(min_h, max_h)
            })
        };

        let pad = ui_tokens::EDGE_PADDING_TIGHT;
        let gap = ui_tokens::CONTENT_GAP;
        let input_gap = (gap * 0.5).max(4.0);
        let footer_inset = ui_tokens::FOOTER_INSET;
        let bar_width = (content_width - pad * 2.0).max(120.0);
        let current_bar: CGRect = msg_send![input_bar, frame];
        let height_same = (current_bar.size.height - desired_h).abs() < 0.5;
        let width_same = (current_bar.size.width - bar_width).abs() < 0.5;
        // Check if the agent scroll bottom inset needs updating (e.g. chip strip
        // toggled). The scroll frame is full-bleed (messages pass beneath the
        // input bar), so the inset — not the frame origin — carries the layout.
        // We compare against the actual inset rather than visibility flags,
        // because setHidden may have already been called (by
        // render_attachment_chips_locked) before we get here — making the
        // visibility flag look "stable" even though the inset hasn't been
        // adjusted yet.
        let scroll_needs_reflow = if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let current_insets: NSEdgeInsets = msg_send![agent_scroll, contentInsets];
            let strip_extra = if let Some(strip_ptr) = state.attachment_chip_strip {
                let strip = strip_ptr as Id;
                let strip_visible: bool = !msg_send![strip, isHidden];
                if strip_visible {
                    CHIP_STRIP_HEIGHT + input_gap
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let expected_bottom = footer_inset + desired_h + input_gap + strip_extra;
            (current_insets.bottom - expected_bottom).abs() > 0.5
        } else {
            false
        };
        if height_same && width_same && !scroll_needs_reflow {
            return;
        }

        // Resize input bar (anchored to bottom).
        let new_bar_frame = CGRect::new(
            &CGPoint::new(pad, footer_inset),
            &CGSize::new(bar_width, desired_h),
        );
        let _: () = msg_send![input_bar, setFrame: new_bar_frame];

        // Resize the input row (attach left, text center, send right).
        let row_layout = crate::ui_helpers::chat_input_row_layout(bar_width, desired_h);
        let text_area_frame = CGRect::new(
            &CGPoint::new(row_layout.text_x, row_layout.text_y),
            &CGSize::new(row_layout.text_width, row_layout.text_height),
        );
        let _: () = msg_send![input_scroll, setFrame: text_area_frame];

        // Recenter buttons vertically.
        let attach_frame = CGRect::new(
            &CGPoint::new(row_layout.attach_x, row_layout.attach_y),
            &CGSize::new(row_layout.button_width, row_layout.button_height),
        );
        let _: () = msg_send![attach_btn, setFrame: attach_frame];
        let send_frame = CGRect::new(
            &CGPoint::new(row_layout.send_x, row_layout.send_y),
            &CGSize::new(row_layout.button_width, row_layout.button_height),
        );
        let _: () = msg_send![send_btn, setFrame: send_frame];

        // Position chip strip above input bar and resize agent scroll view.
        let chip_strip_extra = if let Some(strip_ptr) = state.attachment_chip_strip {
            let strip = strip_ptr as Id;
            let strip_visible: bool = !msg_send![strip, isHidden];
            if strip_visible {
                let strip_y = footer_inset + desired_h + input_gap;
                let strip_frame = CGRect::new(
                    &CGPoint::new(pad, strip_y),
                    &CGSize::new(bar_width, CHIP_STRIP_HEIGHT),
                );
                let _: () = msg_send![strip, setFrame: strip_frame];
                CHIP_STRIP_HEIGHT + input_gap
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Keep the agent scroll full-bleed and track input + chips with the
        // bottom content inset, so messages scroll beneath the input bar while
        // the last bubble stays clear of it.
        if let Some(agent_scroll_ptr) = state.agent_scroll_view {
            let agent_scroll = agent_scroll_ptr as Id;
            let inset_bottom = footer_inset + desired_h + input_gap + chip_strip_extra;
            let top = content_height - gap;
            let new_agent_frame = CGRect::new(
                &CGPoint::new(pad, 0.0),
                &CGSize::new((content_width - pad * 2.0).max(0.0), top.max(0.0)),
            );
            let _: () = msg_send![agent_scroll, setFrame: new_agent_frame];
            let mut agent_insets: NSEdgeInsets = msg_send![agent_scroll, contentInsets];
            agent_insets.bottom = inset_bottom;
            let _: () = msg_send![agent_scroll, setContentInsets: agent_insets];

            if let Some(container_ptr) = state.agent_container {
                let container = container_ptr as Id;
                let container_frame: CGRect = msg_send![container, frame];
                // IMPORTANT: do NOT clamp the document view height to the visible clip height.
                // That disables scrolling and makes long agent replies unscrollable.
                let new_size = CGSize::new(new_agent_frame.size.width, container_frame.size.height);
                let _: () = msg_send![container, setFrameSize: new_size];
            }
        }
    }
}

pub fn create_commit_action_bar(action_handler: Option<usize>) -> Id {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let max_width = 390.0;
        let bar_height = 28.0;

        let bar: Id = msg_send![ns_view, alloc];
        let bar_frame = core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &core_graphics::geometry::CGSize::new(max_width, bar_height),
        );
        let bar: Id = msg_send![bar, initWithFrame: bar_frame];

        let btn_width = 64.0;
        let btn_height = 22.0;
        let gap = 8.0;
        let right_edge = max_width - 8.0;

        // Discard button (left of Commit)
        let discard_x = right_edge - btn_width * 2.0 - gap;
        let discard_btn = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(discard_x, 3.0),
                &core_graphics::geometry::CGSize::new(btn_width, btn_height),
            ),
            "Discard",
            crate::ui_helpers::button_style::SMALL_SQUARE,
        );
        if let Some(handler) = action_handler {
            crate::ui_helpers::button_set_action(
                discard_btn,
                handler as Id,
                sel!(onDiscardMessage:),
            );
        }
        let _: () = msg_send![bar, addSubview: discard_btn];

        // Commit button (rightmost)
        let commit_x = right_edge - btn_width;
        let commit_btn = crate::ui_helpers::create_button(
            core_graphics::geometry::CGRect::new(
                &CGPoint::new(commit_x, 3.0),
                &core_graphics::geometry::CGSize::new(btn_width, btn_height),
            ),
            "Commit",
            crate::ui_helpers::button_style::ROUNDED,
        );
        if let Some(handler) = action_handler {
            crate::ui_helpers::button_set_action(commit_btn, handler as Id, sel!(onCommitMessage:));
        }
        let _: () = msg_send![bar, addSubview: commit_btn];

        bar
    }
}
