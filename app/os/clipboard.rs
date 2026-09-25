// clipboard.rs
//
// Purpose: Provides clipboard operations and paste simulation for macOS
//
// Dependencies: arboard (clipboard access), core-graphics (keyboard simulation)
//
// Key Components:
// - paste_and_restore: Smart paste with clipboard snapshot and restoration
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
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Read the current pasteboard image and encode it as PNG.
///
/// This lives beside the clipboard snapshot/restore code so callers can read a
/// synthetic Cmd+C result *before* restoring the user's previous clipboard.
pub(crate) fn get_image_png_best_effort() -> Option<Vec<u8>> {
    let mut clipboard = Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    let width = u32::try_from(image.width).ok()?;
    let height = u32::try_from(image.height).ok()?;
    let rgba = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())?;
    let mut png_data = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png_data)
            .write_image(
                rgba.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .ok()?;
    }
    (!png_data.is_empty()).then_some(png_data)
}

/// macOS virtual key code for 'V' key
const KEYCODE_V: CGKeyCode = 9;
/// macOS virtual key code for 'C' key
const KEYCODE_C: CGKeyCode = 8;
/// macOS virtual key code for Right Arrow
const KEYCODE_RIGHT_ARROW: CGKeyCode = 124;

/// Delay in milliseconds before restoring the original clipboard content
/// Can be overridden via RESTORE_CLIPBOARD_DELAY_MS environment variable
const DEFAULT_RESTORE_DELAY_MS: u64 = 200;
/// How long an armed transcript stays deliverable before the press is refused.
///
/// Bounded so a command pressed long after dictation cannot paste text the user
/// has forgotten about into whatever window happens to be focused now.
pub const DEFERRED_INSERT_TTL: Duration = Duration::from_secs(120);

/// Serializes explicit clipboard replacements against delayed restores. A new
/// write invalidates every older restore before that write can land.
static CLIPBOARD_RESTORE_EPOCH: Mutex<u64> = Mutex::new(0);

/// A transcript waiting for the user to press the deferred-insert command.
#[derive(Debug, Clone)]
struct DeferredInsertSlot {
    /// Text to paste when the command fires.
    text: String,
    /// Arming instant, measured against [`DEFERRED_INSERT_TTL`].
    armed_at: Instant,
}

/// The single armed transcript. Process-local: arming never touches the system
/// pasteboard, so the user's clipboard survives an insert that never happens.
static DEFERRED_INSERT_SLOT: Mutex<Option<DeferredInsertSlot>> = Mutex::new(None);

/// Why a deferred-insert press did or did not paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredInsertDelivery {
    /// The armed transcript was pasted and the slot consumed.
    Delivered,
    /// Nothing was armed — the press was a no-op.
    NothingToInsert,
    /// The slot outlived [`DEFERRED_INSERT_TTL`]; it is dropped, not pasted.
    Expired,
}

/// [`arm_deferred_insert`] with an injectable arming instant, for tests.
fn arm_deferred_insert_at(text: String, armed_at: Instant) -> bool {
    if text.is_empty() {
        return false;
    }
    let mut slot = DEFERRED_INSERT_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(DeferredInsertSlot { text, armed_at });
    true
}

/// Replace the process-local delivery slot without reading or writing the
/// system clipboard. The newest transcript always wins.
pub fn arm_deferred_insert(text: String) -> bool {
    arm_deferred_insert_at(text, Instant::now())
}

/// Consume the armed slot, or report why there is nothing to paste.
///
/// An expired slot is taken and dropped rather than left behind, so a stale
/// transcript cannot be resurrected by a later press.
fn take_deferred_insert_at(now: Instant) -> Result<String, DeferredInsertDelivery> {
    let mut slot = DEFERRED_INSERT_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(armed) = slot.take() else {
        return Err(DeferredInsertDelivery::NothingToInsert);
    };
    if now.saturating_duration_since(armed.armed_at) >= DEFERRED_INSERT_TTL {
        return Err(DeferredInsertDelivery::Expired);
    }
    Ok(armed.text)
}

/// [`deliver_deferred_insert`] with the clock and paste step injected.
///
/// The seam that lets the arm/expire/deliver-once rules be tested without
/// posting real keyboard events or touching the system pasteboard.
fn deliver_deferred_insert_at<F>(now: Instant, paste: F) -> Result<DeferredInsertDelivery>
where
    F: FnOnce(&str) -> Result<()>,
{
    let text = match take_deferred_insert_at(now) {
        Ok(text) => text,
        Err(outcome) => return Ok(outcome),
    };
    paste(&text)?;
    Ok(DeferredInsertDelivery::Delivered)
}

/// Consume the armed transcript and run the classic snapshot → set → Cmd+V →
/// restore path at the moment the user presses the global command.
pub fn deliver_deferred_insert() -> Result<DeferredInsertDelivery> {
    deliver_deferred_insert_at(Instant::now(), paste_and_restore)
}

/// Permission truth required before posting a synthetic Cmd+V.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntheticPastePreflight {
    /// `CGPreflightPostEventAccess`: may this process post CGEvents at all.
    pub cg_post_event_access: bool,
    /// `AXIsProcessTrusted`: is the app listed under Accessibility.
    pub ax_trusted: bool,
}

impl SyntheticPastePreflight {
    /// Either signal holding allows synthetic event delivery.
    pub(crate) fn can_post_events(self) -> bool {
        self.cg_post_event_access || self.ax_trusted
    }
}

/// Check both macOS signals that govern synthetic keyboard event delivery.
#[cfg(target_os = "macos")]
pub(crate) fn synthetic_paste_preflight() -> SyntheticPastePreflight {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        /// Whether this process may post CGEvents (synthetic keystrokes).
        fn CGPreflightPostEventAccess() -> bool;
        /// Whether Accessibility trusts this process for AX automation.
        fn AXIsProcessTrusted() -> bool;
    }

    // SAFETY: both ApplicationServices functions are process-local, read-only
    // permission probes with no pointer arguments or ownership transfer.
    unsafe {
        SyntheticPastePreflight {
            cg_post_event_access: CGPreflightPostEventAccess(),
            ax_trusted: AXIsProcessTrusted(),
        }
    }
}

/// Off macOS there is no synthetic-paste permission to grant — report denied.
#[cfg(not(target_os = "macos"))]
pub(crate) fn synthetic_paste_preflight() -> SyntheticPastePreflight {
    SyntheticPastePreflight {
        cg_post_event_access: false,
        ax_trusted: false,
    }
}

/// STOP-owned destination identity. It outlives hold-key release and the final wait.
#[derive(Debug)]
pub(crate) struct StopPasteTarget {
    #[cfg(target_os = "macos")]
    identity: Option<stop_target_identity::Identity>,
}

impl StopPasteTarget {
    /// Capture both the foreground process and its focused accessibility element.
    pub(crate) fn capture() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            identity: stop_target_identity::Identity::capture(),
        }
    }

    fn still_focused(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.identity.as_ref().is_some_and(|expected| {
                stop_target_identity::Identity::capture()
                    .as_ref()
                    .is_some_and(|current| expected.same_target(current))
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

#[cfg(target_os = "macos")]
mod stop_target_identity {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::c_void;
    use std::ptr::NonNull;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
        fn AXUIElementCopyAttributeValue(
            element: *mut c_void,
            attribute: *const c_void,
            value: *mut *mut c_void,
        ) -> i32;
        fn AXUIElementGetPid(element: *mut c_void, pid: *mut i32) -> i32;
        fn AXUIElementSetMessagingTimeout(element: *mut c_void, seconds: f32) -> i32;
        fn CFRelease(value: *const c_void);
        fn CFEqual(left: *const c_void, right: *const c_void) -> u8;
    }

    /// Retained AX object identity; no selection text or display label is identity.
    #[derive(Debug)]
    pub(super) struct Identity {
        pid: i32,
        element: NonNull<c_void>,
    }

    // SAFETY: this owns a retained, immutable AX handle. It never exposes the
    // pointer or mutates the element; CF equality and release are thread safe.
    // AX handles represent remote UI objects and are not AppKit view objects.
    unsafe impl Send for Identity {}
    // SAFETY: shared access performs only CFEqual on retained immutable handles.
    unsafe impl Sync for Identity {}

    impl Drop for Identity {
        fn drop(&mut self) {
            // SAFETY: capture owns exactly one Copy-rule reference.
            unsafe { CFRelease(self.element.as_ptr()) };
        }
    }

    fn frontmost_pid() -> Option<i32> {
        // SAFETY: NSWorkspace and NSRunningApplication accessors are read-only;
        // returned objects are borrowed for this call and no pointer escapes.
        unsafe {
            let class = Class::get("NSWorkspace")?;
            let workspace: *mut objc::runtime::Object = msg_send![class, sharedWorkspace];
            if workspace.is_null() {
                return None;
            }
            let app: *mut objc::runtime::Object = msg_send![workspace, frontmostApplication];
            if app.is_null() {
                return None;
            }
            let pid: i32 = msg_send![app, processIdentifier];
            (pid > 0).then_some(pid)
        }
    }

    impl Identity {
        pub(super) fn capture() -> Option<Self> {
            let pid = frontmost_pid()?;
            // SAFETY: both AX Create/Copy results are owned and released exactly
            // once. Output pointers are valid; failed reads never become identity.
            unsafe {
                let application = NonNull::new(AXUIElementCreateApplication(pid))?;
                // A stalled app must not add the default AX RPC timeout to the
                // stop-final wait. An unreadable target resolves to clipboard.
                if AXUIElementSetMessagingTimeout(application.as_ptr(), 0.05) != 0 {
                    CFRelease(application.as_ptr());
                    return None;
                }
                let attribute = CFString::new("AXFocusedUIElement");
                let mut element = std::ptr::null_mut();
                let result = AXUIElementCopyAttributeValue(
                    application.as_ptr(),
                    attribute.as_concrete_TypeRef().cast(),
                    &mut element,
                );
                CFRelease(application.as_ptr());
                let element = NonNull::new(element)?;
                let identity = Self { pid, element };
                let mut element_pid = 0;
                if result != 0
                    || AXUIElementGetPid(element.as_ptr(), &mut element_pid) != 0
                    || element_pid != pid
                    || frontmost_pid() != Some(pid)
                {
                    return None;
                }
                Some(identity)
            }
        }

        pub(super) fn same_target(&self, current: &Self) -> bool {
            self.pid == current.pid
                // SAFETY: both objects retain their Copy-rule AX reference.
                && unsafe { CFEqual(self.element.as_ptr(), current.element.as_ptr()) != 0 }
        }
    }
}

/// Delivery truth for the retained STOP target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopPasteDelivery {
    Pasted,
    CopiedTargetChanged,
}

/// Clipboard replacement precedes the final identity check. No keyboard event
/// or delayed restore is allowed when that check cannot confirm the STOP target.
fn write_stop_paste(
    text: &str,
    write: impl FnOnce(&str) -> Result<u64>,
    target_matches: impl FnOnce() -> bool,
    post_paste: impl FnOnce() -> Result<()>,
    target_changed: impl FnOnce(),
) -> Result<(StopPasteDelivery, u64)> {
    let epoch = write(text)?;
    if !target_matches() {
        target_changed();
        return Ok((StopPasteDelivery::CopiedTargetChanged, epoch));
    }
    post_paste()?;
    Ok((StopPasteDelivery::Pasted, epoch))
}

/// Paste only into the destination retained at STOP. A changed or unreadable
/// destination keeps the entire text on the clipboard and reports copied state.
pub(crate) fn paste_to_stop_target(
    text: &str,
    target: &StopPasteTarget,
) -> Result<StopPasteDelivery> {
    let snapshot = ClipboardSnapshot::capture().ok();
    let (outcome, epoch) = write_stop_paste(
        text,
        set_clipboard_with_epoch,
        || target.still_focused(),
        simulate_cmd_v,
        || {
            warn!(
                event = "stop_paste_target_changed",
                action = "copied",
                text_bytes = text.len(),
                "stop_paste_target_changed"
            );
        },
    )?;
    if outcome == StopPasteDelivery::Pasted {
        // Do not emit a delayed Right Arrow into a destination that may have
        // changed since Cmd+V. The target owns its post-paste selection behavior.
        if let Some(snapshot) = snapshot {
            schedule_clipboard_restore(snapshot, epoch, get_restore_delay());
        }
    }
    Ok(outcome)
}

/// Gets the clipboard restore delay from environment or uses default
fn get_restore_delay() -> Duration {
    let delay_ms = std::env::var("RESTORE_CLIPBOARD_DELAY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RESTORE_DELAY_MS);
    Duration::from_millis(delay_ms)
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

        // An empty snapshot is still a real clipboard state. Clearing here is
        // what makes snapshot -> set -> paste -> restore lossless when the user
        // had nothing on the pasteboard before the deferred insert.
        if self.is_empty() {
            clipboard
                .clear()
                .context("Failed to restore empty clipboard")?;
            debug!("Restored empty clipboard");
            return Ok(());
        }

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
fn set_clipboard_with_epoch(text: &str) -> Result<u64> {
    if text.is_empty() {
        warn!("Attempted to set clipboard with empty text");
        return Ok(*CLIPBOARD_RESTORE_EPOCH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()));
    }

    let mut restore_epoch = CLIPBOARD_RESTORE_EPOCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut clipboard = Clipboard::new().context("Failed to initialize clipboard")?;
    clipboard
        .set_text(text)
        .context("Failed to set clipboard text")?;
    *restore_epoch = restore_epoch.wrapping_add(1);

    debug!("Clipboard set successfully ({} chars)", text.len());
    Ok(*restore_epoch)
}

/// Replace the clipboard contents, discarding the restore epoch.
///
/// For callers that own the clipboard outright. A paste that must hand the
/// clipboard back needs the epoch, so it uses `set_clipboard_with_epoch`.
///
/// # Errors
/// Returns an error if the clipboard cannot be opened or written.
pub fn set_clipboard(text: &str) -> Result<()> {
    set_clipboard_with_epoch(text).map(|_| ())
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

/// No AppKit pasteboard off macOS; callers fall back to content comparison.
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

/// Hand the clipboard back on a background thread once the paste has settled.
///
/// The restore is conditional on `paste_epoch` still being current. Any
/// clipboard write in the meantime — an explicit overlay Copy, a second
/// dictation — bumps the epoch, and this thread then exits without writing, so
/// a delayed restore can never clobber newer content the user is waiting on.
///
/// Returns the handle so tests can join instead of racing the delay.
fn schedule_clipboard_restore(
    snapshot: ClipboardSnapshot,
    paste_epoch: u64,
    delay: Duration,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(delay);
        let restore_epoch = CLIPBOARD_RESTORE_EPOCH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *restore_epoch != paste_epoch {
            debug!(
                paste_epoch,
                current_epoch = *restore_epoch,
                "Skipped stale clipboard restore after a newer clipboard replacement"
            );
            return;
        }

        if let Err(e) = snapshot.restore() {
            warn!("Failed to restore clipboard snapshot: {}", e);
        } else {
            info!("Clipboard snapshot restored");
        }
    })
}

/// Smart paste with configurable clipboard restoration
///
/// This is the lower-level form of [`paste_and_restore`] that lets the caller control
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
                debug!(empty = snap.is_empty(), "Captured clipboard snapshot");
                Some(snap)
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
    let paste_epoch =
        set_clipboard_with_epoch(text).context("Failed to set clipboard for paste")?;
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
        schedule_clipboard_restore(snapshot, paste_epoch, delay);
    }

    Ok(())
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

/// Clipboard snapshot/restore, epoch cancel, and deferred-insert unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn stop_target_switch_during_wait_copies_every_word_without_posting_keys() {
        use std::cell::{Cell, RefCell};

        let stopped_at = Instant::now();
        let target_at_stop = (17, "editor one");
        let switched_at = stopped_at + Duration::from_secs(2);
        let paste_at = stopped_at + Duration::from_secs(8);
        let text = "committed words [untimed preview words]";
        // Also cover a focus move between elements of the same process and an
        // unreadable AX target. Neither proves the original caret still owns V.
        for target_after_switch in [Some((28, "editor two")), Some((17, "editor two")), None] {
            let clipboard = RefCell::new(String::from("old clipboard"));
            let key_posts = Cell::new(0);
            let receipts = RefCell::new(Vec::new());
            let (outcome, _) = write_stop_paste(
                text,
                |complete| {
                    *clipboard.borrow_mut() = complete.to_string();
                    Ok(42)
                },
                || {
                    let current = if paste_at >= switched_at {
                        target_after_switch
                    } else {
                        Some(target_at_stop)
                    };
                    current == Some(target_at_stop)
                },
                || {
                    key_posts.set(key_posts.get() + 1);
                    Ok(())
                },
                || receipts.borrow_mut().push("stop_paste_target_changed"),
            )
            .expect("copy complete stop text");
            assert_eq!(outcome, StopPasteDelivery::CopiedTargetChanged);
            assert_eq!(*clipboard.borrow(), text);
            assert_eq!(key_posts.get(), 0);
            assert_eq!(*receipts.borrow(), ["stop_paste_target_changed"]);
        }
    }

    #[test]
    fn stop_target_is_checked_after_clipboard_write_and_before_any_key() {
        use std::cell::{Cell, RefCell};

        let target_matches = Cell::new(true);
        let actions = RefCell::new(Vec::new());
        let (outcome, _) = write_stop_paste(
            "all words",
            |_| {
                actions.borrow_mut().push("clipboard");
                // Target can change during clipboard work, after the wait ended.
                target_matches.set(false);
                Ok(7)
            },
            || {
                actions.borrow_mut().push("target");
                target_matches.get()
            },
            || {
                actions.borrow_mut().push("key");
                Ok(())
            },
            || actions.borrow_mut().push("receipt"),
        )
        .expect("changed target copy");
        assert_eq!(outcome, StopPasteDelivery::CopiedTargetChanged);
        assert_eq!(*actions.borrow(), ["clipboard", "target", "receipt"]);
    }

    #[test]
    fn unchanged_stop_target_posts_paste_once_and_returns_restore_epoch() {
        use std::cell::Cell;

        let key_posts = Cell::new(0);
        let (outcome, epoch) = write_stop_paste(
            "complete text",
            |_| Ok(19),
            || true,
            || {
                key_posts.set(key_posts.get() + 1);
                Ok(())
            },
            || panic!("unchanged target must not emit changed receipt"),
        )
        .expect("paste unchanged target");
        assert_eq!(outcome, StopPasteDelivery::Pasted);
        assert_eq!(epoch, 19);
        assert_eq!(key_posts.get(), 1);
    }

    /// Round-trip plain text through set_clipboard / get_clipboard when available.
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

    /// Empty set is a soft no-op (warns) and must not error.
    #[test]
    #[serial]
    fn test_empty_clipboard_warning() {
        let _guard = ClipboardTestGuard::capture();
        // Should not panic, just log warning
        let result = set_clipboard("");
        assert!(result.is_ok());
    }

    /// Capture records non-empty text and reports `is_empty() == false`.
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

    /// Restore puts prior text back after an intervening set_clipboard.
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

    /// Empty snapshots clear the pasteboard so deferred insert is lossless.
    #[test]
    #[serial]
    fn empty_clipboard_snapshot_restores_empty_state() {
        let _guard = ClipboardTestGuard::capture();
        let Some(mut clipboard) = skip_if_clipboard_unavailable(
            Clipboard::new().context("initialize clipboard for empty snapshot"),
            "initialize clipboard",
        ) else {
            return;
        };
        let Some(()) = skip_if_clipboard_unavailable(
            clipboard.clear().context("clear clipboard before snapshot"),
            "clear clipboard",
        ) else {
            return;
        };
        let Some(snapshot) =
            skip_if_clipboard_unavailable(ClipboardSnapshot::capture(), "capture empty clipboard")
        else {
            return;
        };
        assert!(snapshot.is_empty());

        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("temporary deferred payload"),
            "set temporary payload",
        ) else {
            return;
        };
        let Some(()) = skip_if_clipboard_unavailable(snapshot.restore(), "restore empty clipboard")
        else {
            return;
        };

        let mut clipboard = Clipboard::new().expect("reopen clipboard after restore");
        assert!(clipboard.get_text().is_err(), "clipboard should be empty");
    }

    /// A newer set bumps the epoch so a delayed restore cannot clobber it.
    #[test]
    #[serial]
    fn degrade_copy_cancels_pending_paste_restore() {
        let _guard = ClipboardTestGuard::capture();
        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("clipboard before paste"),
            "set pre-paste clipboard",
        ) else {
            return;
        };
        let Some(snapshot) =
            skip_if_clipboard_unavailable(ClipboardSnapshot::capture(), "capture pre-paste state")
        else {
            return;
        };
        let Some(paste_epoch) = skip_if_clipboard_unavailable(
            set_clipboard_with_epoch("temporary paste payload"),
            "schedule paste clipboard",
        ) else {
            return;
        };

        let restore = schedule_clipboard_restore(snapshot, paste_epoch, Duration::from_millis(25));
        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("degraded tagged transcript"),
            "replace clipboard with degraded transcript",
        ) else {
            let _ = restore.join();
            return;
        };
        restore.join().expect("restore thread must finish");

        let Some(current) =
            skip_if_clipboard_unavailable(get_clipboard(), "read degraded clipboard")
        else {
            return;
        };
        assert_eq!(current, "degraded tagged transcript");
    }

    /// Arming stores text in-process only; the system pasteboard is untouched.
    #[test]
    #[serial]
    fn deferred_insert_arm_does_not_touch_clipboard() {
        let _guard = ClipboardTestGuard::capture();
        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("user clipboard sentinel"),
            "set deferred insert sentinel",
        ) else {
            return;
        };

        assert!(arm_deferred_insert("tagged transcript".to_string()));

        let Some(current) =
            skip_if_clipboard_unavailable(get_clipboard(), "read deferred insert sentinel")
        else {
            return;
        };
        assert_eq!(current, "user clipboard sentinel");
        let _ = take_deferred_insert_at(Instant::now());
    }

    /// Latest arm wins; second press after delivery reports NothingToInsert.
    #[test]
    #[serial]
    fn deferred_insert_press_delivers_once_and_rearm_replaces_payload() {
        let base = Instant::now();
        assert!(arm_deferred_insert_at("first".to_string(), base));
        assert!(arm_deferred_insert_at("second".to_string(), base));

        let mut delivered = Vec::new();
        let outcome = deliver_deferred_insert_at(base, |text| {
            delivered.push(text.to_string());
            Ok(())
        })
        .expect("deliver armed transcript");

        assert_eq!(outcome, DeferredInsertDelivery::Delivered);
        assert_eq!(delivered, ["second"]);
        assert_eq!(
            deliver_deferred_insert_at(base, |_| Ok(())).expect("empty second press"),
            DeferredInsertDelivery::NothingToInsert
        );
    }

    /// Delivery path restores the user's clipboard after the temporary paste set.
    #[test]
    #[serial]
    fn deferred_insert_press_restores_user_clipboard_after_delivery() {
        let _guard = ClipboardTestGuard::capture();
        let Some(()) = skip_if_clipboard_unavailable(
            set_clipboard("user clipboard before deferred insert"),
            "set clipboard before deferred insert delivery",
        ) else {
            return;
        };
        let base = Instant::now();
        assert!(arm_deferred_insert_at(
            "tagged transcript".to_string(),
            base
        ));

        let outcome = deliver_deferred_insert_at(base, |text| {
            let snapshot = ClipboardSnapshot::capture()?;
            let paste_epoch = set_clipboard_with_epoch(text)?;
            assert_eq!(get_clipboard()?, "tagged transcript");
            schedule_clipboard_restore(snapshot, paste_epoch, Duration::ZERO)
                .join()
                .expect("clipboard restore thread must finish");
            Ok(())
        })
        .expect("deliver deferred transcript through clipboard swap");

        assert_eq!(outcome, DeferredInsertDelivery::Delivered);
        let Some(restored) = skip_if_clipboard_unavailable(
            get_clipboard(),
            "read clipboard after deferred insert delivery",
        ) else {
            return;
        };
        assert_eq!(restored, "user clipboard before deferred insert");
    }

    /// Past TTL yields Expired, never pastes, and consumes the slot.
    #[test]
    #[serial]
    fn deferred_insert_expiry_never_delivers_stale_text() {
        let base = Instant::now();
        assert!(arm_deferred_insert_at("stale".to_string(), base));
        let mut paste_called = false;

        let outcome = deliver_deferred_insert_at(base + DEFERRED_INSERT_TTL, |_| {
            paste_called = true;
            Ok(())
        })
        .expect("expired press is not an error");

        assert_eq!(outcome, DeferredInsertDelivery::Expired);
        assert!(!paste_called);
        assert_eq!(
            deliver_deferred_insert_at(base + DEFERRED_INSERT_TTL, |_| Ok(()))
                .expect("expired slot was consumed"),
            DeferredInsertDelivery::NothingToInsert
        );
    }

    /// Soft-skip when the host has no pasteboard; panic on unexpected errors.
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

    /// Detect arboard "not supported with the current system configuration" hosts.
    fn is_clipboard_unavailable(error: &anyhow::Error) -> bool {
        format!("{error:#}").contains("not supported with the current system configuration")
    }

    /// RAII guard that restores the operator clipboard after each serial test.
    struct ClipboardTestGuard(Option<ClipboardSnapshot>);

    impl ClipboardTestGuard {
        /// Best-effort capture; missing clipboard yields a no-op Drop.
        fn capture() -> Self {
            Self(ClipboardSnapshot::capture().ok())
        }
    }

    impl Drop for ClipboardTestGuard {
        /// Restore the pre-test snapshot if capture succeeded.
        fn drop(&mut self) {
            if let Some(snapshot) = &self.0 {
                let _ = snapshot.restore();
            }
        }
    }
}
