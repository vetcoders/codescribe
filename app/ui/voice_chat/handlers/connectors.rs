//! Connector attachments: fetch remote content as local attachment files.
//!
//! GitHub blob references and arbitrary URLs, fetched on a background thread
//! and registered as attachments on the main thread.

use super::*;

// ═══════════════════════════════════════════════════════════
// Connector Handlers (GitHub, URL)
// ═══════════════════════════════════════════════════════════

/// Show an input dialog and fetch a file from GitHub, adding it as an attachment.
pub extern "C" fn on_attach_github(_this: &Object, _cmd: Sel, _sender: Id) {
    let input = show_text_input_dialog(
        "Attach from GitHub",
        "Enter a GitHub file URL or spec:\n\
         \u{2022} https://github.com/owner/repo/blob/branch/path\n\
         \u{2022} owner/repo@branch:path/to/file",
        "https://github.com/...",
    );
    let Some(input) = input else { return };
    let input = input.trim().to_string();
    if input.is_empty() {
        return;
    }

    let Some(gh_ref) = codescribe_core::connectors::github::parse_github_ref(&input) else {
        show_error_alert(
            "Invalid GitHub reference",
            &format!("Could not parse: {input}"),
        );
        return;
    };

    // Fetch in background thread, then add attachment on main thread.
    //
    // P2.4 (DEFERRED — cross-cut): ideally this would reuse the main
    // multi-threaded tokio runtime (bin: worker_threads = 4) via a cached
    // `Handle`. But this is an objc `extern "C"` action firing on the AppKit
    // main thread, which is NOT a tokio worker, so `Handle::current()` would
    // panic and no global `Handle` is exposed to the voice_chat domain.
    // Caching one requires a startup-side `OnceLock<Handle>` in bin/controller
    // (GROUP controller/concurrency). Until that lands, keep the per-fetch
    // current-thread runtime: connector fetches are rare and short-lived.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let msg = format!("Failed to start async runtime: {e}");
                Queue::main().exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
                return;
            }
        };
        let result = rt.block_on(async {
            let token = codescribe_core::connectors::github::load_github_token();
            codescribe_core::connectors::github::fetch_github_blob(&gh_ref, token.as_deref()).await
        });
        match result {
            Ok((data, filename)) => {
                match codescribe_core::attachment::AttachmentStore::save_fetched(
                    &data, &filename, "gh",
                ) {
                    Ok(path) => {
                        Queue::main().exec_async(move || {
                            let (btn_ptr, count, names, skipped) = {
                                let mut state =
                                    OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                                let mut skipped = Vec::new();
                                let attachment = Attachment::from_path(
                                    path,
                                    AttachmentSource::Connector("github".into()),
                                );
                                push_attachment_if_allowed(&mut state, attachment, &mut skipped);
                                state.attachments_last_sent = None;
                                // P2.11: safe to render under the held lock —
                                // render_attachment_chips is run-loop-free (see
                                // its doc-comment in send.rs); no nested run-loop
                                // can re-enter and re-lock OVERLAY_STATE.
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
                        });
                    }
                    Err(e) => {
                        let msg = format!("Failed to save: {e}");
                        Queue::main()
                            .exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                Queue::main().exec_async(move || show_error_alert("GitHub Fetch Error", &msg));
            }
        }
    });
}

/// Show an input dialog and fetch content from a URL, adding it as an attachment.
pub extern "C" fn on_attach_url(_this: &Object, _cmd: Sel, _sender: Id) {
    let input = show_text_input_dialog(
        "Attach from URL",
        "Enter a URL to fetch as attachment context:",
        "https://...",
    );
    let Some(input) = input else { return };
    let input = input.trim().to_string();
    if input.is_empty() {
        return;
    }
    if !codescribe_core::connectors::web::looks_like_url(&input) {
        show_error_alert("Invalid URL", "URL must start with http:// or https://");
        return;
    }

    // Fetch in background thread, then add attachment on main thread.
    //
    // P2.4 (DEFERRED — cross-cut): see `on_attach_github` above. Reusing the
    // main runtime needs a startup-cached `Handle` (GROUP controller/concurrency);
    // `Handle::current()` panics on the AppKit main thread. Per-fetch runtime
    // retained until that cross-cut lands.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let msg = format!("Failed to start async runtime: {e}");
                Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
                return;
            }
        };
        let result = rt.block_on(codescribe_core::connectors::web::fetch_url_as_text(&input));
        match result {
            Ok((text, title)) => {
                let display_name = if title.is_empty() {
                    "webpage.txt".to_string()
                } else {
                    format!("{}.txt", title.chars().take(40).collect::<String>())
                };
                match codescribe_core::attachment::AttachmentStore::save_fetched(
                    text.as_bytes(),
                    &display_name,
                    "url",
                ) {
                    Ok(path) => {
                        Queue::main().exec_async(move || {
                            let (btn_ptr, count, names, skipped) = {
                                let mut state =
                                    OVERLAY_STATE.lock().unwrap_or_else(|e| e.into_inner());
                                let mut skipped = Vec::new();
                                let att = Attachment::with_kind(
                                    path,
                                    codescribe_core::attachment::AttachmentKind::UrlSnapshot,
                                    AttachmentSource::Connector("web".into()),
                                );
                                push_attachment_if_allowed(&mut state, att, &mut skipped);
                                state.attachments_last_sent = None;
                                // P2.11: safe under the held lock — chip render is
                                // run-loop-free (see render_attachment_chips_locked
                                // doc-comment in send.rs).
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
                        });
                    }
                    Err(e) => {
                        let msg = format!("Failed to save: {e}");
                        Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                Queue::main().exec_async(move || show_error_alert("URL Fetch Error", &msg));
            }
        }
    });
}
