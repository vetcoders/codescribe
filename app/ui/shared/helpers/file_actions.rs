use objc::runtime::Class;
use objc::{msg_send, sel, sel_impl};

use super::{Id, ns_string};

pub fn pick_files_open_panel(title: &str) -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    unsafe {
        let Some(ns_open_panel) = Class::get("NSOpenPanel") else {
            return Vec::new();
        };
        let panel: Id = msg_send![ns_open_panel, openPanel];
        if panel.is_null() {
            return Vec::new();
        }

        let _: () = msg_send![panel, setTitle: ns_string(title)];
        let _: () = msg_send![panel, setCanChooseFiles: true];
        let _: () = msg_send![panel, setCanChooseDirectories: false];
        let _: () = msg_send![panel, setAllowsMultipleSelection: true];

        // Prefer predictable prompt text (keeps UX clear).
        let _: () = msg_send![panel, setPrompt: ns_string("Attach")];

        // runModal returns NSModalResponse (NSModalResponseOK == 1).
        let resp: isize = msg_send![panel, runModal];
        if resp != 1 {
            return Vec::new();
        }

        let urls: Id = msg_send![panel, URLs];
        if urls.is_null() {
            return Vec::new();
        }

        let count: usize = msg_send![urls, count];
        let mut out = Vec::with_capacity(count);
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
            if s.is_empty() {
                continue;
            }
            out.push(std::path::PathBuf::from(s));
        }
        out
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        Vec::new()
    }
}

/// Open a file in the default editor (TextEdit, etc.)
pub fn open_file_in_editor(path: &std::path::Path) -> bool {
    // Most reliable approach in the app-bundle environment: call `/usr/bin/open`.
    // NSWorkspace sometimes reports success but doesn't surface the editor window. `open -e`
    // (TextEdit) is predictable and works without PATH.
    #[cfg(target_os = "macos")]
    {
        use std::time::Duration;
        use tracing::{info, warn};

        let path = path.to_path_buf();
        if !path.exists() {
            warn!(
                "open_file_in_editor: path does not exist: {}",
                path.display()
            );
            return false;
        }

        let open_via_nsworkspace_textedit = || -> bool {
            unsafe {
                let ns_workspace = match Class::get("NSWorkspace") {
                    Some(c) => c,
                    None => return false,
                };
                let workspace: Id = msg_send![ns_workspace, sharedWorkspace];
                if workspace.is_null() {
                    return false;
                }

                let path_str = path.to_string_lossy();
                let ns_path = ns_string(&path_str);
                let app = ns_string("TextEdit");

                // Prefer explicit app open (avoids "Open…" panel / wrong default handler).
                let ok: bool = msg_send![workspace, openFile: ns_path withApplication: app];
                info!("NSWorkspace openFile:withApplication(TextEdit) ok={}", ok);
                ok
            }
        };

        let run_open = |args: &[&str]| -> bool {
            let out = std::process::Command::new("/usr/bin/open")
                .args(args)
                .arg(&path)
                .output();
            match out {
                Ok(out) => {
                    let code = out.status.code().unwrap_or(-1);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !stderr.trim().is_empty() {
                        info!(
                            "open {:?} exit={} stderr={}",
                            args,
                            code,
                            stderr.trim().replace('\n', "\\n")
                        );
                    } else {
                        info!("open {:?} exit={}", args, code);
                    }
                    out.status.success()
                }
                Err(e) => {
                    warn!("open {:?} failed to spawn: {}", args, e);
                    false
                }
            }
        };

        let activate_textedit_best_effort = || {
            // Try to bring TextEdit to the foreground without requiring Automation permissions
            // (osascript can trigger a prompt / fail silently).
            unsafe {
                let ns_running_app = match Class::get("NSRunningApplication") {
                    Some(c) => c,
                    None => return,
                };
                let bundle_id = ns_string("com.apple.TextEdit");
                let apps: Id =
                    msg_send![ns_running_app, runningApplicationsWithBundleIdentifier: bundle_id];
                if apps.is_null() {
                    return;
                }

                let count: usize = msg_send![apps, count];
                if count == 0 {
                    return;
                }

                // NSApplicationActivateAllWindows (1) | NSApplicationActivateIgnoringOtherApps (2)
                let opts: u64 = 3;
                for i in 0..count {
                    let app: Id = msg_send![apps, objectAtIndex: i];
                    if app.is_null() {
                        continue;
                    }
                    let ok: bool = msg_send![app, activateWithOptions: opts];
                    info!("TextEdit activateWithOptions result={}", ok);
                }
            }
        };

        // Force TextEdit and try to surface it; otherwise it can open "somewhere" (another Space)
        // and look like a no-op from the user's POV.
        // Prefer `open -a TextEdit <file>` (explicit app + file). Fallback to `-e` if needed.
        if open_via_nsworkspace_textedit() || run_open(&["-a", "TextEdit"]) || run_open(&["-e"]) {
            // Give launch a moment so NSRunningApplication can see the process.
            std::thread::sleep(Duration::from_millis(75));
            activate_textedit_best_effort();
            return true;
        }
        if run_open(&["-t"]) || run_open(&[]) {
            return true;
        }
    }

    unsafe {
        let ns_workspace = Class::get("NSWorkspace").unwrap();
        let workspace: Id = msg_send![ns_workspace, sharedWorkspace];

        let path_str = path.to_string_lossy();
        let ns_path = ns_string(&path_str);

        let ok: bool = msg_send![workspace, openFile: ns_path];
        if ok {
            return true;
        }

        // Fallback: open via file:// URL (some apps prefer this path).
        let ns_url = Class::get("NSURL").unwrap();
        let url: Id = msg_send![ns_url, fileURLWithPath: ns_path];
        if url.is_null() {
            // last resort below (shell open)
        } else {
            let ok2: bool = msg_send![workspace, openURL: url];
            if ok2 {
                return true;
            }
        }
    }

    let _ = path;
    false
}

/// List draft files from a directory, sorted by modification time (newest first)
pub fn list_draft_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|ext| ext == "txt" || ext == "md")
        })
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();

    // Sort by modification time, newest first
    files.sort_by_key(|b| std::cmp::Reverse(b.1));

    files.into_iter().map(|(path, _)| path).collect()
}
