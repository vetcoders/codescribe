//! Selection/context capture for assistive mode (macOS)
//!
//! POC goal:
//! - If user has selected text in the frontmost app, include it as context for Assistive mode.
//! - Avoid clipboard pollution by snapshot+restore.
//! - Best-effort only: failure should never break recording/transcription.

use std::time::Duration;

use tracing::{debug, info, warn};

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
/// - `ASSISTIVE_CONTEXT_MAX_CHARS` (default: 20000)
/// - `ASSISTIVE_CONTEXT_INCLUDE_APP` (default: 1)
/// - `ASSISTIVE_CONTEXT_COPY_DELAY_MS` (default: 150)
/// - `ASSISTIVE_CONTEXT_COPY_FALLBACK` (default: auto) - enable Cmd+C fallback when AX selection is unavailable
pub fn capture_assistive_context() -> AssistiveContext {
    // Unit tests should not trigger osascript / clipboard / event simulation.
    if cfg!(test) {
        return AssistiveContext::default();
    }

    if !env_flag("ASSISTIVE_CONTEXT_ENABLED", true) {
        return AssistiveContext::default();
    }

    let max_chars = env_usize("ASSISTIVE_CONTEXT_MAX_CHARS", 20000);
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
        info!("Assistive context: frontmost is CodeScribe, skipping selection capture");
        return AssistiveContext {
            frontmost_app,
            selected_text: None,
        };
    }

    let selected_text =
        selected_text_from_frontmost(max_chars, copy_delay_ms, frontmost_app.as_deref());

    info!(
        "Assistive context: app={:?}, selected_chars={}",
        frontmost_app.as_deref().unwrap_or("(none)"),
        selected_text
            .as_ref()
            .map(|s| s.chars().count())
            .unwrap_or(0)
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
fn prefer_copy_fallback_for_app(frontmost_app: Option<&str>) -> bool {
    let app = frontmost_app.unwrap_or("").trim().to_lowercase();
    matches!(
        app.as_str(),
        "safari"
            | "google chrome"
            | "google chrome beta"
            | "arc"
            | "brave browser"
            | "firefox"
            | "microsoft edge"
            | "orion"
            | "vivaldi"
    )
}

#[cfg(target_os = "macos")]
fn selected_text_from_frontmost(
    max_chars: usize,
    copy_delay_ms: u64,
    frontmost_app: Option<&str>,
) -> Option<String> {
    // Prefer Accessibility selection if available (doesn't depend on clipboard).
    //
    // IMPORTANT: Only trust AXSelectedText when AXSelectedTextRange confirms a real
    // selection exists (length > 0). Some apps (e.g. Notes) return the FULL text
    // content from AXSelectedText when nothing is selected, which would cause inline
    // edit to replace the entire document.
    let sel_len = crate::ui::get_selected_text_length();
    info!(
        "Selection capture: AX range length={:?}, app={:?}",
        sel_len,
        frontmost_app.unwrap_or("(none)")
    );
    if matches!(sel_len, Some(n) if n > 0) {
        if let Some(selected) = crate::ui::get_selected_text(max_chars) {
            info!("Selection capture: AX text OK ({} chars, range={})",
                selected.chars().count(), sel_len.unwrap_or(0));
            return Some(selected);
        }
    }
    info!("Selection capture: AX range empty/unavailable (len={:?}), trying Cmd+C fallback",
        sel_len);

    // Cmd+C fallback is enabled by default for:
    // 1. Web browsers where AX selection is unreliable (notably Safari).
    // 2. Any app where AXSelectedTextRange is unavailable (sel_len == None),
    //    because AXSelectedText alone can't distinguish selection from full content.
    // The explicit env flag still overrides this behavior.
    // We snapshot+restore to avoid clipboard pollution and treat "unchanged clipboard" as no selection.
    let range_unavailable = sel_len.is_none();
    let fallback_default = range_unavailable || prefer_copy_fallback_for_app(frontmost_app);
    if !env_flag("ASSISTIVE_CONTEXT_COPY_FALLBACK", fallback_default) {
        if matches!(sel_len, Some(0)) {
            debug!("Assistive context: selection length is 0; Cmd+C fallback disabled");
        } else {
            info!(
                "Selection capture: Cmd+C fallback disabled for {:?}",
                frontmost_app.unwrap_or("(none)")
            );
        }
        return None;
    }
    if matches!(sel_len, Some(0)) {
        debug!("Assistive context: AX range length=0; trying Cmd+C fallback");
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

    if let Some(snapshot) = snapshot
        && let Err(e) = snapshot.restore()
    {
        debug!("Assistive context: clipboard restore failed: {}", e);
    }

    copied = copied.trim().to_string();
    if copied.is_empty() {
        return None;
    }

    // If clipboard didn't change, treat as "no selection" to avoid leaking arbitrary clipboard data.
    if let Some(prev) = prev_text
        && copied == prev.trim()
    {
        debug!("Assistive context: clipboard unchanged; treating as no selection");
        return None;
    }

    let copied_chars = copied.chars().count();
    if copied_chars > max_chars {
        copied = copied.chars().take(max_chars).collect();
        copied.push('…');
    }

    Some(copied)
}

/// Replace the currently selected text in the frontmost application.
///
/// Uses direct AX attribute write (`AXSelectedText`). No clipboard pollution.
/// Returns `Ok("ax")` on success, `Err` when target is read-only / unsupported.
///
/// Caller should fall back to overlay display when this fails (e.g. terminal output).
#[cfg(target_os = "macos")]
pub fn replace_selected_text(new_text: &str) -> Result<&'static str, String> {
    use tracing::info;

    if new_text.is_empty() {
        return Err("replace_selected_text called with empty text".into());
    }

    match crate::ui::set_selected_text(new_text) {
        Ok(true) => {
            info!(
                "replace_selected_text: AX write succeeded ({} chars)",
                new_text.len()
            );
            Ok("ax")
        }
        Ok(false) => {
            debug!("replace_selected_text: AX write unsupported (target likely read-only)");
            Err("target_not_editable".into())
        }
        Err(e) => {
            debug!("replace_selected_text: AX write error: {}", e);
            Err(format!("ax_write_failed: {}", e))
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn replace_selected_text(_new_text: &str) -> Result<&'static str, String> {
    Err("replace_selected_text is only supported on macOS".into())
}

#[cfg(not(target_os = "macos"))]
fn selected_text_from_frontmost(
    _max_chars: usize,
    _copy_delay_ms: u64,
    _frontmost_app: Option<&str>,
) -> Option<String> {
    None
}
