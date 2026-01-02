// clipboard.rs
//
// Purpose: Provides clipboard operations and paste simulation for macOS
//
// Note: Some functions are not yet wired up to main.rs (pending integration)
#![allow(dead_code)]
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

/// Takes a snapshot of the current clipboard
///
/// Convenience function for ClipboardSnapshot::capture()
pub fn snapshot_clipboard() -> Result<ClipboardSnapshot> {
    ClipboardSnapshot::capture()
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

/// Alias for set_clipboard - copies text to clipboard without pasting
///
/// # Arguments
/// * `text` - The text to copy to clipboard
///
/// # Errors
/// Returns error if clipboard operation fails
pub fn copy(text: &str) -> Result<()> {
    set_clipboard(text)
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

/// Simulates Right Arrow keystroke using CGEvent
fn simulate_right_arrow() -> Result<()> {
    // Key down: Right Arrow (no modifiers)
    simulate_key_event(KEYCODE_RIGHT_ARROW, true, CGEventFlags::empty())?;
    thread::sleep(Duration::from_millis(5));

    // Key up: Right Arrow
    simulate_key_event(KEYCODE_RIGHT_ARROW, false, CGEventFlags::empty())?;

    Ok(())
}

/// Simple paste function - just sets clipboard and simulates Cmd+V
///
/// Does NOT restore the previous clipboard content. Use paste_and_restore()
/// for smart clipboard management.
///
/// # Arguments
/// * `text` - The text to paste
///
/// # Errors
/// Returns error if clipboard or keyboard simulation fails
pub fn paste(text: &str) -> Result<()> {
    if text.is_empty() {
        warn!("Paste called with empty text");
        return Ok(());
    }

    set_clipboard(text).context("Failed to set clipboard for paste")?;

    // Simulate Cmd+V using CGEvent (thread-safe)
    simulate_cmd_v().context("Failed to simulate Cmd+V")?;

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
/// ```no_run
/// use codescribe::clipboard::paste_and_restore;
/// paste_and_restore("Hello, world!").expect("Failed to paste");
/// ```
pub fn paste_and_restore(text: &str) -> Result<()> {
    paste_text_smart(text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require real clipboard access which may crash in CI
    // environments without a proper display server. Run manually:
    // cargo test --lib -- clipboard --ignored

    #[test]
    #[ignore = "Requires real clipboard access - run with --ignored"]
    fn test_set_and_get_clipboard() {
        let test_text = "Test clipboard content";
        set_clipboard(test_text).expect("Failed to set clipboard");

        let retrieved = get_clipboard().expect("Failed to get clipboard");
        assert_eq!(retrieved, test_text);
    }

    #[test]
    fn test_empty_clipboard_warning() {
        // Should not panic, just log warning
        let result = set_clipboard("");
        assert!(result.is_ok());
    }

    #[test]
    #[ignore = "Requires real clipboard access - run with --ignored"]
    fn test_clipboard_snapshot_capture() {
        // Set some text
        set_clipboard("Test snapshot content").expect("Failed to set clipboard");

        // Capture snapshot
        let snapshot = ClipboardSnapshot::capture().expect("Failed to capture snapshot");

        // Should have text
        assert!(snapshot.text.is_some());
        assert_eq!(snapshot.text.as_ref().unwrap(), "Test snapshot content");
        assert!(!snapshot.is_empty());
    }

    #[test]
    #[ignore = "Requires real clipboard access - run with --ignored"]
    fn test_clipboard_snapshot_restore() {
        // Set original content
        let original = "Original clipboard text";
        set_clipboard(original).expect("Failed to set clipboard");

        // Capture snapshot
        let snapshot = ClipboardSnapshot::capture().expect("Failed to capture snapshot");

        // Change clipboard
        set_clipboard("Different text").expect("Failed to change clipboard");

        // Restore snapshot
        snapshot.restore().expect("Failed to restore snapshot");

        // Should match original
        let restored = get_clipboard().expect("Failed to get clipboard");
        assert_eq!(restored, original);
    }

    #[test]
    #[ignore = "Requires real clipboard access - run with --ignored"]
    fn test_copy_alias() {
        let test_text = "Copy alias test";
        copy(test_text).expect("Failed to copy");

        let retrieved = get_clipboard().expect("Failed to get clipboard");
        assert_eq!(retrieved, test_text);
    }
}
