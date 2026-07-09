// clipboard.rs
//
// Purpose: Provides clipboard operations and paste simulation for macOS
//
// Note: Some functions are not yet wired up to main.rs (pending integration)
//
// Dependencies: arboard (clipboard access), core-graphics (keyboard simulation)
//
// Key Components:
// - paste_and_restore: Smart paste with clipboard snapshot and restoration
// - paste_text: Simple paste with optional restore
// - copy: Copy text to clipboard
// - paste: Paste without simulation
// - ClipboardSnapshot: Captures and restores all clipboard formats
//
// Design Rationale: Uses arboard for cross-platform clipboard access and
// CGEvent (via core-graphics) for keyboard event simulation. This avoids
// the TSMGetInputSourceProperty crash on macOS 26.2 that occurs with enigo
// when called from background threads. Implements clipboard save/restore
// pattern to preserve user's clipboard after paste operations.

use anyhow::{Context, Result};
use arboard::{Clipboard, ImageData};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::thread;
use std::time::Duration;
use tracing::{debug, info, warn};

/// macOS virtual key code for 'V' key
const KEYCODE_V: CGKeyCode = 9;
/// macOS virtual key code for 'C' key
const KEYCODE_C: CGKeyCode = 8;
/// macOS virtual key code for Right Arrow
const KEYCODE_RIGHT_ARROW: CGKeyCode = 124;

/// Delay in milliseconds before restoring the original clipboard content
/// Can be overridden via RESTORE_CLIPBOARD_DELAY_MS environment variable
const DEFAULT_RESTORE_DELAY_MS: u64 = 200;

/// Gets the clipboard restore delay from environment or uses default
fn get_restore_delay() -> Duration {
    let delay_ms = std::env::var("RESTORE_CLIPBOARD_DELAY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RESTORE_DELAY_MS);
    Duration::from_millis(delay_ms)
}

/// Checks if clipboard restore is enabled via environment variable
fn is_restore_enabled() -> bool {
    std::env::var("RESTORE_CLIPBOARD")
        .ok()
        .map(|v| {
            let lower = v.to_lowercase();
            !matches!(lower.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(true) // Default: enabled
}

/// Clipboard snapshot containing all available formats
///
/// Captures text, HTML, and image data from the clipboard so it can be
/// restored after a paste operation. Only non-empty formats are captured.
#[derive(Debug, Clone)]
pub struct ClipboardSnapshot {
    /// Plain text content (if available)
    pub text: Option<String>,
    /// HTML content (if available)
    pub html: Option<String>,
    /// Image data (if available)
    pub image: Option<ImageData<'static>>,
}

impl ClipboardSnapshot {
    /// Creates a new snapshot of the current clipboard state
    ///
    /// Attempts to capture all available formats. If a format is not available
    /// or fails to retrieve, it will be None in the snapshot.
    ///
    /// # Errors
    /// Returns error if clipboard initialization fails
    pub fn capture() -> Result<Self> {
        let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;

        // Try to get text
        let text = clipboard.get_text().ok();
        if let Some(ref t) = text {
            debug!("Captured clipboard text ({} chars)", t.len());
        }

        // Try to get HTML (arboard may not support this on all platforms)
        let html = None; // arboard 3.x doesn't expose get_html publicly

        // Try to get image
        let image = clipboard.get_image().ok();
        if image.is_some() {
            debug!("Captured clipboard image");
        }

        Ok(Self { text, html, image })
    }

    /// Restores this snapshot to the clipboard
    ///
    /// Restores all captured formats back to the clipboard. If multiple formats
    /// were captured, they will all be restored.
    ///
    /// # Errors
    /// Returns error if clipboard operations fail
    pub fn restore(&self) -> Result<()> {
        let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;

        // Restore text if we have it
        if let Some(ref text) = self.text {
            clipboard
                .set_text(text)
                .context("Failed to restore clipboard text")?;
            debug!("Restored clipboard text ({} chars)", text.len());
        }

        // Restore HTML if we have it (arboard may not support this)
        if let Some(ref _html) = self.html {
            // arboard 3.x set_html requires both HTML and alt text
            // We'll skip this for now as we can't capture HTML reliably
        }

        // Restore image if we have it
        if let Some(ref image) = self.image {
            clipboard
                .set_image(image.clone())
                .context("Failed to restore clipboard image")?;
            debug!("Restored clipboard image");
        }

        Ok(())
    }

    /// Checks if the snapshot contains any data
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none()
    }
}

/// Sets the clipboard content without simulating paste
///
/// # Arguments
/// * `text` - The text to copy to clipboard
///
/// # Errors
/// Returns error if clipboard operation fails
pub fn set_clipboard(text: &str) -> Result<()> {
    if text.is_empty() {
        warn!("Attempted to set clipboard with empty text");
        return Ok(());
    }

    let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;
    clipboard
        .set_text(text)
        .context("Failed to set clipboard text")?;

    debug!("Clipboard set successfully ({} chars)", text.len());
    Ok(())
}

/// Gets the current clipboard content
///
/// # Errors
/// Returns error if clipboard operation fails or clipboard is empty
pub fn get_clipboard() -> Result<String> {
    let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;
    let text = clipboard
        .get_text()
        .context("Failed to get clipboard text")?;

    debug!("Retrieved clipboard content ({} chars)", text.len());
    Ok(text)
}

/// Simulates a key press using CGEvent (thread-safe, no TSM issues)
///
/// # Arguments
/// * `keycode` - macOS virtual key code
/// * `key_down` - true for key down, false for key up
/// * `flags` - modifier flags (e.g., CGEventFlags::CGEventFlagCommand)
fn simulate_key_event(keycode: CGKeyCode, key_down: bool, flags: CGEventFlags) -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .ok()
        .context("Failed to create CGEventSource")?;

    let event = CGEvent::new_keyboard_event(source, keycode, key_down)
        .ok()
        .context("Failed to create keyboard event")?;

    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);

    Ok(())
}

/// Simulates Cmd+V keystroke using CGEvent
///
/// This is thread-safe and doesn't use TSM APIs that crash on macOS 26.2.
fn simulate_cmd_v() -> Result<()> {
    let cmd_flag = CGEventFlags::CGEventFlagCommand;

    // Key down: V with Cmd modifier
    simulate_key_event(KEYCODE_V, true, cmd_flag)?;
    thread::sleep(Duration::from_millis(10));

    // Key up: V with Cmd modifier
    simulate_key_event(KEYCODE_V, false, cmd_flag)?;

    Ok(())
}

/// Reads `NSPasteboard.generalPasteboard.changeCount`.
///
/// The change count is a monotonically increasing token that bumps every time
/// the pasteboard is written to, regardless of *what* was written. Comparing it
/// across a synthetic Cmd+C is a content-agnostic way to detect whether the copy
/// actually wrote anything — it eliminates the false-negative where a selection
/// happens to equal the previous clipboard text, and the false-positive where
/// the previous clipboard held a non-text payload (e.g. an image) and stale text
/// gets mistaken for "the selection".
///
/// Returns `None` when the AppKit binding is unavailable (non-macOS, or class
/// lookup fails) so callers can fall back to content comparison.
#[cfg(target_os = "macos")]
pub(crate) fn pasteboard_change_count() -> Option<i64> {
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};

    // SAFETY: NSPasteboard.generalPasteboard returns a shared singleton and
    // changeCount is a simple integer accessor; no ownership transfer occurs.
    unsafe {
        let cls = Class::get("NSPasteboard")?;
        let pasteboard: *mut objc::runtime::Object = msg_send![cls, generalPasteboard];
        if pasteboard.is_null() {
            return None;
        }
        let count: i64 = msg_send![pasteboard, changeCount];
        Some(count)
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn pasteboard_change_count() -> Option<i64> {
    None
}

/// Simulates Cmd+C keystroke using CGEvent
///
/// Used for best-effort selection capture (clipboard snapshot+restore).
pub(crate) fn simulate_cmd_c() -> Result<()> {
    let cmd_flag = CGEventFlags::CGEventFlagCommand;

    // Key down: C with Cmd modifier
    simulate_key_event(KEYCODE_C, true, cmd_flag)?;
    thread::sleep(Duration::from_millis(10));

    // Key up: C with Cmd modifier
    simulate_key_event(KEYCODE_C, false, cmd_flag)?;

    Ok(())
}

/// Simulates Right Arrow keystroke using CGEvent
fn simulate_right_arrow() -> Result<()> {
    // Key down: Right Arrow (no modifiers)
    simulate_key_event(KEYCODE_RIGHT_ARROW, true, CGEventFlags::empty())?;
    thread::sleep(Duration::from_millis(5));

    // Key up: Right Arrow
    simulate_key_event(KEYCODE_RIGHT_ARROW, false, CGEventFlags::empty())?;

    Ok(())
}

/// Smart paste with configurable clipboard restoration
///
/// This is a more flexible version of paste_text that allows you to control
/// whether the clipboard is restored. Useful when you want to paste multiple
/// times without fighting clipboard restoration.
///
/// # Arguments
/// * `text` - The text to paste
/// * `restore` - Whether to restore the clipboard after pasting
///
/// # Errors
/// Returns error if clipboard or keyboard simulation fails
pub fn paste_text_smart(text: &str, restore: bool) -> Result<()> {
    if text.is_empty() {
        warn!("Paste called with empty text");
        return Ok(());
    }

    info!(
        "Smart pasting text: '{}...' ({} chars), restore={}",
        &text.chars().take(50).collect::<String>(),
        text.len(),
        restore
    );

    // 1. Save current clipboard content if restore is requested
    let snapshot = if restore {
        match ClipboardSnapshot::capture() {
            Ok(snap) => {
                if !snap.is_empty() {
                    debug!("Captured clipboard snapshot");
                    Some(snap)
                } else {
                    debug!("Clipboard snapshot is empty, skipping restore");
                    None
                }
            }
            Err(e) => {
                warn!("Could not capture clipboard snapshot: {}", e);
                None
            }
        }
    } else {
        None
    };

    // 2. Set clipboard to new text
    set_clipboard(text).context("Failed to set clipboard for paste")?;
    info!("Text successfully copied to clipboard");

    // 3. Simulate Cmd+V keypress using CGEvent (thread-safe)
    simulate_cmd_v().context("Failed to simulate Cmd+V")?;
    info!("Command+V keypress simulated successfully");

    // 4. Wait for paste to settle
    thread::sleep(Duration::from_millis(50));

    // 5. Simulate Right Arrow to deselect pasted text
    simulate_right_arrow().context("Failed to simulate Right Arrow")?;
    debug!("Cleared selection (moved cursor to end)");

    // 6. Optional: restore clipboard snapshot after delay
    if let Some(snapshot) = snapshot {
        let delay = get_restore_delay();
        thread::spawn(move || {
            thread::sleep(delay);
            if let Err(e) = snapshot.restore() {
                warn!("Failed to restore clipboard snapshot: {}", e);
            } else {
                info!("Clipboard snapshot restored");
            }
        });
    }

    Ok(())
}

/// Pastes text into the currently active application
///
/// This function implements a sophisticated paste operation:
/// 1. Saves current clipboard content (if restore is enabled)
/// 2. Sets clipboard to new text
/// 3. Simulates Cmd+V keypress
/// 4. Waits briefly for paste to complete
/// 5. Simulates Right Arrow to deselect pasted text
/// 6. Restores original clipboard content after configurable delay
///
/// The clipboard restore can be disabled by setting RESTORE_CLIPBOARD=0
/// The restore delay can be configured via RESTORE_CLIPBOARD_DELAY_MS
///
/// # Arguments
/// * `text` - The text to paste
///
/// # Errors
/// Returns error if clipboard or keyboard simulation fails
///
/// # Platform Support
/// Currently macOS-only. Uses Cmd modifier for paste simulation.
pub fn paste_text(text: &str) -> Result<()> {
    paste_text_smart(text, is_restore_enabled())
}

/// Pastes text and always restores the previous clipboard content
///
/// This is the highest-level paste function that:
/// 1. Captures a complete snapshot of the clipboard (text, HTML, images)
/// 2. Pastes the provided text
/// 3. Restores the snapshot after a configurable delay
///
/// Use this when you want to paste text without disrupting the user's clipboard.
///
/// # Arguments
/// * `text` - The text to paste
///
/// # Errors
/// Returns error if clipboard or keyboard simulation fails
///
/// # Example
/// ```ignore
/// use codescribe::clipboard::paste_and_restore;
/// paste_and_restore("Hello, world!").expect("Failed to paste");
/// ```
pub fn paste_and_restore(text: &str) -> Result<()> {
    paste_text_smart(text, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_set_and_get_clipboard() {
        let _guard = ClipboardTestGuard::capture();
        let test_text = "Test clipboard content";
        let Some(()) = skip_if_clipboard_unavailable(set_clipboard(test_text), "set clipboard")
        else {
            return;
        };

        let Some(retrieved) = skip_if_clipboard_unavailable(get_clipboard(), "get clipboard")
        else {
            return;
        };
        assert_eq!(retrieved, test_text);
    }

    #[test]
    #[serial]
    fn test_empty_clipboard_warning() {
        let _guard = ClipboardTestGuard::capture();
        // Should not panic, just log warning
        let result = set_clipboard("");
        assert!(result.is_ok());
    }

    #[test]
    #[serial]
    fn test_clipboard_snapshot_capture() {
        let _guard = ClipboardTestGuard::capture();
        // Set some text
        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("Test snapshot content"),
            "set snapshot clipboard",
        ) else {
            return;
        };

        // Capture snapshot
        let Some(snapshot) =
            skip_if_clipboard_unavailable(ClipboardSnapshot::capture(), "capture snapshot")
        else {
            return;
        };

        // Should have text
        assert!(snapshot.text.is_some());
        assert_eq!(snapshot.text.as_ref().unwrap(), "Test snapshot content");
        assert!(!snapshot.is_empty());
    }

    #[test]
    #[serial]
    fn test_clipboard_snapshot_restore() {
        let _guard = ClipboardTestGuard::capture();
        // Set original content
        let original = "Original clipboard text";
        let Some(()) =
            skip_if_clipboard_unavailable(set_clipboard(original), "set original clipboard")
        else {
            return;
        };

        // Capture snapshot
        let Some(snapshot) =
            skip_if_clipboard_unavailable(ClipboardSnapshot::capture(), "capture snapshot")
        else {
            return;
        };

        // Change clipboard
        let Some(()) =
            skip_if_clipboard_unavailable(set_clipboard("Different text"), "change clipboard")
        else {
            return;
        };

        // Restore snapshot
        let Some(()) = skip_if_clipboard_unavailable(snapshot.restore(), "restore snapshot") else {
            return;
        };

        // Should match original
        let Some(restored) = skip_if_clipboard_unavailable(get_clipboard(), "get clipboard") else {
            return;
        };
        assert_eq!(restored, original);
    }

    fn skip_if_clipboard_unavailable<T>(result: Result<T>, action: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) if is_clipboard_unavailable(&error) => {
                eprintln!("skipping clipboard integration test: {action}: {error:#}");
                None
            }
            Err(error) => panic!("{action}: {error:#}"),
        }
    }

    fn is_clipboard_unavailable(error: &anyhow::Error) -> bool {
        format!("{error:#}").contains("not supported with the current system configuration")
    }

    struct ClipboardTestGuard(Option<ClipboardSnapshot>);

    impl ClipboardTestGuard {
        fn capture() -> Self {
            Self(ClipboardSnapshot::capture().ok())
        }
    }

    impl Drop for ClipboardTestGuard {
        fn drop(&mut self) {
            if let Some(snapshot) = &self.0 {
                let _ = snapshot.restore();
            }
        }
    }
}
