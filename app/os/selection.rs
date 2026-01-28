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

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// Capture best-effort context for assistive mode.
///
/// Env knobs (POC):
/// - `ASSISTIVE_CONTEXT_ENABLED` (default: 1)
/// - `ASSISTIVE_CONTEXT_MAX_CHARS` (default: 5000)
/// - `ASSISTIVE_CONTEXT_INCLUDE_APP` (default: 1)
/// - `ASSISTIVE_CONTEXT_COPY_DELAY_MS` (default: 150)
/// - `ASSISTIVE_CONTEXT_COPY_FALLBACK` (default: 0) - enable Cmd+C fallback when AX selection is unavailable
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
    let copy_delay_ms = env_u64("ASSISTIVE_CONTEXT_COPY_DELAY_MS", 150);

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

    let selected_text = selected_text_from_frontmost(max_chars, copy_delay_ms);

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

/// Capture only the frontmost app name (no selection, no clipboard).
///
/// This is used to make paste actions (⇲) target the right app even when we're not in Assistive
/// selection mode.
pub fn capture_frontmost_app_only() -> AssistiveContext {
    if cfg!(test) {
        return AssistiveContext::default();
    }

    if !env_flag("ASSISTIVE_CONTEXT_ENABLED", true) {
        return AssistiveContext::default();
    }

    let include_app = env_flag("ASSISTIVE_CONTEXT_INCLUDE_APP", true);
    let frontmost_app = if include_app {
        frontmost_app_name()
    } else {
        None
    };

    AssistiveContext {
        frontmost_app,
        selected_text: None,
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
fn selected_text_from_frontmost(max_chars: usize, copy_delay_ms: u64) -> Option<String> {
    // Prefer Accessibility selection if available (doesn't depend on clipboard).
    //
    // Some apps report `AXSelectedTextRange.length == 0` even when `AXSelectedText` is non-empty,
    // so we do *not* early-return on length==0 before checking `AXSelectedText`.
    let sel_len = crate::ui::get_selected_text_length();
    if let Some(selected) = crate::ui::get_selected_text(max_chars) {
        return Some(selected);
    }

    // If we can reliably detect that selection length is zero, treat as "no selection" and
    // never use any fallback that might touch the clipboard.
    if matches!(sel_len, Some(0)) {
        debug!("Assistive context: selection length is 0; skipping Cmd+C fallback");
        return None;
    }

    // Cmd+C fallback is enabled by default for Selection mode. Some apps don't expose AX APIs.
    // We snapshot+restore to avoid clipboard pollution and treat "unchanged clipboard" as no selection.
    //
    // NOTE: Default is OFF because Cmd+C can be surprising/privacy-sensitive (it touches clipboard)
    // and some users explicitly want selection context to come only from AXSelectedText.
    if !env_flag("ASSISTIVE_CONTEXT_COPY_FALLBACK", false) {
        return None;
    }

    // Fallback: snapshot clipboard + Cmd+C + restore.
    // This can fail in some apps and can mis-detect "no selection" when clipboard doesn't change.
    let snapshot = ClipboardSnapshot::capture().ok();
    let prev_text = snapshot.as_ref().and_then(|s| s.text.clone());

    if let Err(e) = clipboard::simulate_cmd_c() {
        warn!("Assistive context: failed to simulate Cmd+C: {}", e);
        return None;
    }

    std::thread::sleep(Duration::from_millis(copy_delay_ms));

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

    // If clipboard didn't change, treat as "no selection" to avoid leaking arbitrary clipboard data.
    if let Some(prev) = prev_text {
        if copied == prev.trim() {
            debug!("Assistive context: clipboard unchanged; treating as no selection");
            return None;
        }
    }

    if copied.len() > max_chars {
        copied.truncate(max_chars);
        copied.push('…');
    }

    Some(copied)
}

#[cfg(not(target_os = "macos"))]
fn selected_text_from_frontmost(_max_chars: usize, _copy_delay_ms: u64) -> Option<String> {
    None
}
