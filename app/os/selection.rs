//! Selection/context capture for assistive mode (macOS)
//!
//! POC goal:
//! - If user has selected text in the frontmost app, include it as context for Assistive mode.
//! - Avoid clipboard pollution by snapshot+restore.
//! - Best-effort only: failure should never break recording/transcription.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tracing::{debug, warn};

use crate::os::clipboard::{self, ClipboardSnapshot};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssistiveContext {
    pub frontmost_app: Option<String>,
    pub selected_text: Option<String>,
}

#[derive(Debug, Clone)]
struct TimedAssistiveContext {
    captured_at: std::time::Instant,
    ctx: AssistiveContext,
}

fn recent_assistive_context_store() -> &'static Mutex<Option<TimedAssistiveContext>> {
    static STORE: OnceLock<Mutex<Option<TimedAssistiveContext>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(None))
}

/// Store the latest assistive context for short-lived follow-up prompts in chat.
pub fn store_recent_assistive_context(ctx: &AssistiveContext) {
    let mut guard = recent_assistive_context_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *guard = Some(TimedAssistiveContext {
        captured_at: std::time::Instant::now(),
        ctx: ctx.clone(),
    });
}

/// Return the latest assistive context if it is still fresh.
pub fn get_recent_assistive_context(max_age: Duration) -> Option<AssistiveContext> {
    let mut guard = recent_assistive_context_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = guard.as_ref()?;

    if entry.captured_at.elapsed() <= max_age {
        return Some(entry.ctx.clone());
    }

    // Drop stale data to avoid leaking old context into later prompts.
    *guard = None;
    None
}

#[cfg(test)]
fn clear_recent_assistive_context_for_tests() {
    let mut guard = recent_assistive_context_store()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *guard = None;
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
    capture_assistive_context_with_prior_frontmost(None)
}

/// Capture assistive context while preferring the app that was frontmost before
/// CodeScribe UI could activate.
pub fn capture_assistive_context_with_prior_frontmost(
    prior_frontmost_app: Option<String>,
) -> AssistiveContext {
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

    let current_frontmost_app = if include_app {
        frontmost_app_name()
    } else {
        None
    };
    capture_assistive_context_from_parts(
        current_frontmost_app,
        prior_frontmost_app,
        |frontmost_app, should_restore_prior_app| {
            capture_selected_text_with_effective_frontmost(
                max_chars,
                copy_delay_ms,
                frontmost_app,
                should_restore_prior_app,
            )
        },
    )
}

/// Capture only the frontmost app name (no selection, no clipboard).
///
/// This is used to make paste actions (⇲) target the right app even when we're not in Assistive
/// selection mode.
pub fn capture_frontmost_app_only() -> AssistiveContext {
    capture_frontmost_app_only_with_prior_frontmost(None)
}

/// Capture only the frontmost app name, using a pre-overlay app if CodeScribe is
/// currently frontmost.
pub fn capture_frontmost_app_only_with_prior_frontmost(
    prior_frontmost_app: Option<String>,
) -> AssistiveContext {
    if cfg!(test) {
        return AssistiveContext::default();
    }

    if !env_flag("ASSISTIVE_CONTEXT_ENABLED", true) {
        return AssistiveContext::default();
    }

    let include_app = env_flag("ASSISTIVE_CONTEXT_INCLUDE_APP", true);
    let current_frontmost_app = if include_app {
        frontmost_app_name()
    } else {
        None
    };
    let (frontmost_app, _) =
        resolve_effective_frontmost_app(current_frontmost_app, prior_frontmost_app);

    AssistiveContext {
        frontmost_app,
        selected_text: None,
    }
}

/// Best-effort app activation by localized app name.
///
/// Used to recover focus before synthetic paste when frontmost temporarily flips to CodeScribe.
#[cfg(target_os = "macos")]
pub fn activate_app_by_name(app_name: &str) -> bool {
    use std::process::Command;

    let app_name = app_name.trim();
    if app_name.is_empty() || app_name.eq_ignore_ascii_case("codescribe") {
        return false;
    }

    let escaped = app_name.replace('\\', "\\\\").replace('\"', "\\\"");
    let script = format!("tell application \"{}\" to activate", escaped);

    match Command::new("osascript").args(["-e", &script]).output() {
        Ok(out) => {
            if out.status.success() {
                true
            } else {
                debug!(
                    "App activation failed for '{}': exit={:?}",
                    app_name,
                    out.status.code()
                );
                false
            }
        }
        Err(e) => {
            debug!("App activation failed for '{}': {}", app_name, e);
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn activate_app_by_name(_app_name: &str) -> bool {
    false
}

fn normalized_app_name(app_name: Option<String>) -> Option<String> {
    app_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn is_codescribe_app(app_name: &str) -> bool {
    app_name.trim().eq_ignore_ascii_case("codescribe")
}

fn resolve_effective_frontmost_app(
    current_frontmost_app: Option<String>,
    prior_frontmost_app: Option<String>,
) -> (Option<String>, bool) {
    let current_frontmost_app = normalized_app_name(current_frontmost_app);
    let prior_frontmost_app =
        normalized_app_name(prior_frontmost_app).filter(|app_name| !is_codescribe_app(app_name));

    let should_use_prior = match (
        current_frontmost_app.as_deref(),
        prior_frontmost_app.as_deref(),
    ) {
        (Some(current), Some(_)) if is_codescribe_app(current) => true,
        (None, Some(_)) => true,
        _ => false,
    };

    if should_use_prior {
        (prior_frontmost_app, true)
    } else {
        (current_frontmost_app, false)
    }
}

fn capture_assistive_context_from_parts(
    current_frontmost_app: Option<String>,
    prior_frontmost_app: Option<String>,
    selected_text_reader: impl FnOnce(Option<&str>, bool) -> Option<String>,
) -> AssistiveContext {
    let (frontmost_app, should_restore_prior_app) =
        resolve_effective_frontmost_app(current_frontmost_app, prior_frontmost_app);

    // Avoid capturing from ourselves (frontmost can temporarily become CodeScribe)
    if frontmost_app.as_deref().is_some_and(is_codescribe_app) {
        debug!("Assistive context: frontmost is CodeScribe, skipping selection capture");
        return AssistiveContext {
            frontmost_app,
            selected_text: None,
        };
    }

    let selected_text = selected_text_reader(frontmost_app.as_deref(), should_restore_prior_app);

    debug!(
        "Assistive context captured (app_present={}, selected_chars={})",
        frontmost_app.is_some(),
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

fn capture_selected_text_with_effective_frontmost(
    max_chars: usize,
    copy_delay_ms: u64,
    frontmost_app: Option<&str>,
    should_restore_prior_app: bool,
) -> Option<String> {
    if should_restore_prior_app && let Some(app_name) = frontmost_app {
        if activate_app_by_name(app_name) {
            std::thread::sleep(Duration::from_millis(copy_delay_ms.min(80)));
        } else {
            debug!("Assistive context: prior app activation failed before selection capture");
        }
    }

    selected_text_from_frontmost(max_chars, copy_delay_ms, frontmost_app)
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

    if !selected_text.is_empty() {
        out.push_str("ZAZNACZONY_TEKST:\n<<<\n");
        out.push_str(selected_text);
        out.push_str("\n>\n");
    } else {
        out.push_str("ZAZNACZONY_TEKST: brak dostępnego zaznaczenia.\n");
    }

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
    // Some apps report `AXSelectedTextRange.length == 0` even when `AXSelectedText` is non-empty,
    // so we do *not* early-return on length==0 before checking `AXSelectedText`.
    let sel_len = crate::ui::get_selected_text_length();
    if let Some(selected) = crate::ui::get_selected_text(max_chars) {
        return Some(selected);
    }

    // Cmd+C fallback is enabled by default for web browsers where AX selection is unreliable
    // (notably Safari). The explicit env flag still overrides this behavior.
    // We snapshot+restore to avoid clipboard pollution and treat "unchanged clipboard" as no selection.
    let fallback_default = prefer_copy_fallback_for_app(frontmost_app);
    if !env_flag("ASSISTIVE_CONTEXT_COPY_FALLBACK", fallback_default) {
        if matches!(sel_len, Some(0)) {
            debug!("Assistive context: selection length is 0; Cmd+C fallback disabled");
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

#[cfg(not(target_os = "macos"))]
fn selected_text_from_frontmost(
    _max_chars: usize,
    _copy_delay_ms: u64,
    _frontmost_app: Option<&str>,
) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn effective_frontmost_prefers_prior_when_codescribe_is_current() {
        let (app, should_restore) = resolve_effective_frontmost_app(
            Some("CodeScribe".to_string()),
            Some("Terminal".to_string()),
        );

        assert_eq!(app.as_deref(), Some("Terminal"));
        assert!(should_restore);
    }

    #[test]
    fn assistive_capture_uses_prior_frontmost_after_overlay_activation() {
        let ctx = capture_assistive_context_from_parts(
            Some("CodeScribe".to_string()),
            Some("Terminal".to_string()),
            |frontmost_app, should_restore_prior_app| {
                assert_eq!(frontmost_app, Some("Terminal"));
                assert!(should_restore_prior_app);
                Some("selected terminal text".to_string())
            },
        );

        assert_eq!(ctx.frontmost_app.as_deref(), Some("Terminal"));
        assert_eq!(ctx.selected_text.as_deref(), Some("selected terminal text"));
        let input = build_assistive_input("opisz zaznaczenie", &ctx);
        assert!(input.contains("selected terminal text"));
    }

    #[test]
    fn assistive_input_handles_missing_selection_without_empty_context_block() {
        let ctx = AssistiveContext {
            frontmost_app: Some("GitHub Desktop".to_string()),
            selected_text: None,
        };

        let input = build_assistive_input("kontynuuj bez selekcji", &ctx);

        assert!(input.contains("INSTRUKCJA_UŻYTKOWNIKA"));
        assert!(input.contains("kontynuuj bez selekcji"));
        assert!(input.contains("ZAZNACZONY_TEKST: brak dostępnego zaznaczenia."));
        assert!(!input.contains("ZAZNACZONY_TEKST:\n<<<\n\n>"));
        assert!(input.contains("frontmost_app: GitHub Desktop"));
    }

    #[test]
    fn effective_frontmost_preserves_codescribe_guard_without_prior_app() {
        let (app, should_restore) =
            resolve_effective_frontmost_app(Some("CodeScribe".to_string()), None);

        assert_eq!(app.as_deref(), Some("CodeScribe"));
        assert!(!should_restore);
    }

    #[test]
    #[serial]
    fn recent_assistive_context_roundtrips_while_fresh() {
        clear_recent_assistive_context_for_tests();

        let ctx = AssistiveContext {
            frontmost_app: Some("Safari".to_string()),
            selected_text: Some("selected".to_string()),
        };
        store_recent_assistive_context(&ctx);

        assert_eq!(
            get_recent_assistive_context(Duration::from_secs(1)),
            Some(ctx)
        );
    }

    #[test]
    #[serial]
    fn stale_recent_assistive_context_is_cleared() {
        clear_recent_assistive_context_for_tests();

        let ctx = AssistiveContext {
            frontmost_app: Some("CodeScribe".to_string()),
            selected_text: Some("old".to_string()),
        };
        store_recent_assistive_context(&ctx);

        assert_eq!(get_recent_assistive_context(Duration::ZERO), None);
        assert_eq!(
            get_recent_assistive_context(Duration::from_secs(1)),
            None,
            "stale entry should be cleared from the cache"
        );
    }
}
