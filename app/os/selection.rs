//! Selection/context capture for assistive mode (macOS)
//!
//! POC goal:
//! - If user has selected text in the frontmost app, include it as context for Assistive mode.
//! - Avoid clipboard pollution by snapshot+restore.
//! - Best-effort only: failure should never break recording/transcription.

use std::time::Duration;

use tracing::{debug, warn};

use crate::os::clipboard::{self, ClipboardSnapshot};

#[derive(Debug, Clone, Default)]
pub struct AssistiveContext {
    pub frontmost_app: Option<String>,
    pub selected_text: Option<String>,
}

fn env_flag(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let v = v.to_lowercase();
            !matches!(v.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

/// Capture best-effort context for assistive mode.
///
/// Env knobs (POC):
/// - `ASSISTIVE_CONTEXT_ENABLED` (default: 1)
/// - `ASSISTIVE_CONTEXT_MAX_CHARS` (default: 5000)
/// - `ASSISTIVE_CONTEXT_INCLUDE_APP` (default: 1)
pub fn capture_assistive_context() -> AssistiveContext {
    // Unit tests should not trigger osascript / clipboard / event simulation.
    if cfg!(test) {
        return AssistiveContext::default();
    }

    if !env_flag("ASSISTIVE_CONTEXT_ENABLED", true) {
        return AssistiveContext::default();
    }

    let max_chars = env_usize("ASSISTIVE_CONTEXT_MAX_CHARS", 5000);
    let include_app = env_flag("ASSISTIVE_CONTEXT_INCLUDE_APP", true);

    let frontmost_app = if include_app {
        frontmost_app_name()
    } else {
        None
    };

    // Avoid capturing from ourselves (frontmost can temporarily become CodeScribe)
    if matches!(
        frontmost_app.as_deref(),
        Some("CodeScribe") | Some("codescribe")
    ) {
        debug!("Assistive context: frontmost is CodeScribe, skipping selection capture");
        return AssistiveContext {
            frontmost_app,
            selected_text: None,
        };
    }

    let selected_text = selected_text_from_frontmost(max_chars);

    debug!(
        "Assistive context captured (app_present={}, selected_chars={})",
        frontmost_app.is_some(),
        selected_text.as_ref().map(|s| s.len()).unwrap_or(0)
    );

    AssistiveContext {
        frontmost_app,
        selected_text,
    }
}

/// Build the LLM input for assistive mode, including optional selection context.
pub fn build_assistive_input(user_voice_text: &str, ctx: &AssistiveContext) -> String {
    let instruction = user_voice_text.trim();
    let selected_text = ctx.selected_text.as_deref().unwrap_or("").trim();
    let frontmost_app = ctx.frontmost_app.as_deref().unwrap_or("").trim();

    let mut out = String::new();

    out.push_str("INSTRUKCJA_UŻYTKOWNIKA:\n<<<\n");
    out.push_str(instruction);
    out.push_str("\n>\n\n");

    out.push_str("ZAZNACZONY_TEKST:\n<<<\n");
    out.push_str(selected_text);
    out.push_str("\n>\n");

    if !frontmost_app.is_empty() {
        out.push_str("\nKONTEKST:\n- frontmost_app: ");
        out.push_str(frontmost_app);
        out.push('\n');
    }

    out
}

#[cfg(target_os = "macos")]
fn frontmost_app_name() -> Option<String> {
    use std::process::Command;

    // This is best-effort. It may fail if System Events is restricted.
    let output = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to name of first application process whose frontmost is true"#,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(target_os = "macos"))]
fn frontmost_app_name() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn selected_text_from_frontmost(max_chars: usize) -> Option<String> {
    // Prefer Accessibility selection if available (doesn't depend on clipboard).
    if let Some(selected) = crate::ui::get_selected_text(max_chars) {
        return Some(selected);
    }

    // Fallback: snapshot clipboard + Cmd+C + restore.
    // This can fail in some apps and can mis-detect "no selection" when clipboard doesn't change.
    let snapshot = ClipboardSnapshot::capture().ok();

    if let Err(e) = clipboard::simulate_cmd_c() {
        warn!("Assistive context: failed to simulate Cmd+C: {}", e);
        return None;
    }

    std::thread::sleep(Duration::from_millis(80));

    let mut copied = match clipboard::get_clipboard() {
        Ok(t) => t,
        Err(e) => {
            debug!("Assistive context: clipboard read failed: {}", e);
            String::new()
        }
    };

    if let Some(snapshot) = snapshot {
        if let Err(e) = snapshot.restore() {
            debug!("Assistive context: clipboard restore failed: {}", e);
        }
    }

    copied = copied.trim().to_string();
    if copied.is_empty() {
        return None;
    }

    if copied.len() > max_chars {
        copied.truncate(max_chars);
        copied.push('…');
    }

    Some(copied)
}

#[cfg(not(target_os = "macos"))]
fn selected_text_from_frontmost(_max_chars: usize) -> Option<String> {
    None
}
