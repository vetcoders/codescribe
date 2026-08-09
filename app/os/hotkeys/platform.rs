//! Platform binding for the hotkey listener: a CoreGraphics event tap on
//! macOS, and a warning-only stub everywhere else.
//!
//! This is the only file in the hotkey stack that talks to the OS. It decodes
//! CGEvents into [`HotkeyDetectorInput`] and hands them to the pure state
//! machine in [`super::detector`], which is why the gesture rules can be tested
//! without Accessibility permission — the untestable part is confined here.
//!
//! Two properties are load-bearing and easy to break:
//!
//! - **The tap is listen-only.** It observes events and cannot suppress them,
//!   so the callback's return value is ignored by CoreGraphics.
//! - **Teardown is a swap race, not a lock.** Every CoreFoundation handle lives
//!   in an `AtomicPtr` inside `RuntimeControl`; whoever swaps a non-null value
//!   out owns the teardown. That is what keeps `shutdown()` and `Drop` from
//!   double-invalidating the same port.
//!
//! Both `cfg` branches export the same five-item surface
//! ([`HotkeyRuntime`], [`start_listener`], [`enable`], [`disable`],
//! [`is_enabled`]) so callers never need a `cfg` of their own.

use super::config::{get_hotkey_runtime_config, get_mode_hotkey_bindings};
use super::detector::{
    HotkeyDetector, HotkeyDetectorInput, HotkeyEvent, HotkeyModifierSnapshot, HotkeyPhysicalKey,
};
use crossbeam_channel::Sender;
use std::time::{Duration, Instant};

// --- macOS CGEventTap Implementation using raw bindings ---

/// The real implementation: a session-level CGEventTap driven on its own
/// thread by a CFRunLoop.
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    // CGEvent types and flags
    //
    // Hand-written aliases for the CoreGraphics/CoreFoundation ABI. They are
    // opaque pointers and plain integers rather than newtypes so the `extern`
    // declarations below match the C signatures exactly, with no repr risk.

    /// Opaque `CGEventRef` — a single keyboard event, owned by the callback's
    /// caller for the callback's duration only.
    type CGEventRef = *mut c_void;
    /// Opaque `CGEventTapProxy` — unused here, the tap is listen-only.
    type CGEventTapProxy = *mut c_void;
    /// Opaque `CFMachPortRef` — the event tap port; retained, must be released.
    type CFMachPortRef = *mut c_void;
    /// Opaque `CFRunLoopSourceRef` — retained, must be released.
    type CFRunLoopSourceRef = *mut c_void;
    /// Opaque `CFRunLoopRef` — **not** owned; `CFRunLoopGetCurrent` does not
    /// retain, so this one is never released.
    type CFRunLoopRef = *mut c_void;

    /// Discriminant of a CGEvent (key down, key up, flags changed, or a
    /// tap-disabled sentinel).
    type CGEventType = u32;
    /// Bitfield of active modifier keys.
    type CGEventFlags = u64;
    /// Selector for `CGEventGetIntegerValueField`.
    type CGEventField = u32;

    // CGEventType values
    /// A non-modifier key went down.
    const K_CG_EVENT_KEY_DOWN: CGEventType = 10;
    /// A non-modifier key was released.
    const K_CG_EVENT_KEY_UP: CGEventType = 11;
    /// A modifier changed state — carries every hold and double-tap gesture.
    const K_CG_EVENT_FLAGS_CHANGED: CGEventType = 12;

    // CGEventType "tap disabled" sentinels. CoreGraphics emits these (the two
    // highest u32 values) when it forcibly disables a tap — either because a
    // listen-only callback was too slow or because of user input during a
    // sensitive sequence. They live in <CoreGraphics/CGEvent.h> as stable ABI
    // constants, named there `kCGEventTapDisabledByTimeout` and
    // `kCGEventTapDisabledByUserInput`.
    /// Tap killed because our callback took too long to return.
    const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: CGEventType = 0xFFFF_FFFE;
    /// Tap killed by user input during a sensitive sequence.
    const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: CGEventType = 0xFFFF_FFFF;

    // CGEventFlags masks
    /// Control held.
    const K_CG_EVENT_FLAG_MASK_CONTROL: CGEventFlags = 0x00040000;
    /// Shift held.
    const K_CG_EVENT_FLAG_MASK_SHIFT: CGEventFlags = 0x00020000;
    /// Option held — CoreGraphics calls it "alternate".
    const K_CG_EVENT_FLAG_MASK_ALTERNATE: CGEventFlags = 0x00080000; // Option key
    /// Command held.
    const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;
    /// Fn (Globe) held.
    const K_CG_EVENT_FLAG_MASK_SECONDARY_FN: CGEventFlags = 0x00800000;

    // CGEventField for keycode
    /// Field selector that yields the virtual keycode of a keyboard event.
    const K_CG_KEYBOARD_EVENT_KEYCODE: CGEventField = 9;

    // macOS virtual keycodes for Option keys
    /// Left Option — the formatting-mode double-tap side.
    const K_VK_OPTION: i64 = 58; // Left Option
    /// Right Option — the assistive-mode double-tap side.
    const K_VK_RIGHT_OPTION: i64 = 61; // Right Option
    // macOS virtual keycodes for Control keys
    /// Left Control.
    const K_VK_CONTROL: i64 = 59; // Left Control
    /// Right Control.
    const K_VK_RIGHT_CONTROL: i64 = 62; // Right Control
    /// Fn / Globe.
    const K_VK_FUNCTION: i64 = 63; // Fn (Globe)
    /// Space — half of the Show-Agent chord.
    const K_VK_SPACE: i64 = 49;
    /// V — half of the Insert-Here chord.
    const K_VK_V: i64 = 9;

    // CGEventTap constants
    /// Tap at session scope: all events for this login session.
    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    /// Insert at the head of the tap chain.
    const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    /// Observe only — the tap cannot modify or swallow events, and the
    /// callback's return value is ignored.
    const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;

    // Callback type
    /// Signature CoreGraphics expects for an event tap callback.
    type CGEventTapCallBack = extern "C" fn(
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        /// Create an event tap. Returns null when Accessibility permission is
        /// missing — the single most common failure on a fresh install.
        /// The returned port is retained and must be released.
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        /// Arm or disarm a tap. Also used to *re-arm* after macOS disables one.
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        /// Whether the tap is currently armed; checked once at startup because
        /// creation can succeed while enabling is denied.
        fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
        /// Read the modifier bitfield of an event.
        fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
        /// Read one integer field of an event (here: the virtual keycode).
        fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        /// Wrap the tap port as a run loop source. Retained — must be released.
        fn CFMachPortCreateRunLoopSource(
            allocator: *const c_void,
            port: CFMachPortRef,
            order: i64,
        ) -> CFRunLoopSourceRef;
        /// Tear down a mach port so it stops delivering.
        fn CFMachPortInvalidate(port: CFMachPortRef);

        /// The calling thread's run loop. Does **not** retain, so the result is
        /// never released.
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        /// Attach a source to a run loop in the given mode.
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
        /// Tear down a run loop source.
        fn CFRunLoopSourceInvalidate(source: CFRunLoopSourceRef);
        /// Block the current thread, dispatching events until stopped.
        fn CFRunLoopRun();
        /// Ask a run loop to return from `CFRunLoopRun`.
        fn CFRunLoopStop(rl: CFRunLoopRef);
        /// Nudge a run loop so it notices a pending stop instead of sleeping on.
        fn CFRunLoopWakeUp(rl: CFRunLoopRef);
        /// Release a retained CoreFoundation object.
        fn CFRelease(cf: *const c_void);

        /// Mode set that keeps the tap live during menu tracking and drags.
        static kCFRunLoopCommonModes: *const c_void;
    }

    /// Everything the C callback needs, reached through the tap's `user_info`
    /// pointer.
    ///
    /// Boxed and owned by [`EventTapResources`] so the address stays stable for
    /// the tap's whole life; the callback casts `user_info` straight back to
    /// `&mut HotkeyState`.
    struct HotkeyState {
        detector: HotkeyDetector,
        tx: Sender<HotkeyEvent>,
        /// Shared runtime handle so the callback can read the live tap port
        /// (`control.tap`) to re-arm it after macOS disables the tap. The
        /// callback only ever *reads* this pointer — never invalidates or
        /// frees it — so ownership/teardown in `request_stop`/`Drop` is intact.
        control: Arc<RuntimeControl>,
    }

    impl HotkeyState {
        /// Fresh state with a default detector.
        fn new(tx: Sender<HotkeyEvent>, control: Arc<RuntimeControl>) -> Self {
            Self {
                detector: HotkeyDetector::default(),
                tx,
                control,
            }
        }
    }

    /// Whether a listener is live. Process-wide because there is exactly one
    /// event tap; guarded by [`RunningGuard`].
    static RUNNING: AtomicBool = AtomicBool::new(false);
    /// Soft on/off switch read by the callback on every event.
    ///
    /// Separate from `RUNNING` on purpose: disabling makes the tap ignore
    /// events without tearing it down, so re-enabling costs nothing and does
    /// not re-prompt for permission.
    static ENABLED: AtomicBool = AtomicBool::new(true);

    /// RAII token proving this process owns the single listener slot.
    ///
    /// Held by [`HotkeyRuntime`]; releasing it on drop is what lets a
    /// subsequent `start_listener` succeed after a shutdown.
    struct RunningGuard;

    impl RunningGuard {
        /// Claim the listener slot, or fail if one is already running.
        fn acquire() -> Result<Self, String> {
            if RUNNING.swap(true, Ordering::SeqCst) {
                return Err("Hotkey listener already running".to_string());
            }
            Ok(Self)
        }
    }

    impl Drop for RunningGuard {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }

    /// Shared teardown rendezvous between the owning thread, the worker, and
    /// the C callback.
    ///
    /// Each CoreFoundation handle lives in an `AtomicPtr` so ownership can be
    /// claimed by swap rather than by lock: whoever swaps out a non-null value
    /// is responsible for invalidating and releasing it, and everyone else sees
    /// null and no-ops. That is the mechanism preventing the double-invalidate
    /// crash when `shutdown()` and `Drop for EventTapResources` race.
    #[derive(Default)]
    struct RuntimeControl {
        stop_requested: AtomicBool,
        tap: AtomicPtr<c_void>,
        source: AtomicPtr<c_void>,
        run_loop: AtomicPtr<c_void>,
    }

    impl RuntimeControl {
        /// Whether teardown has been asked for.
        fn is_stop_requested(&self) -> bool {
            self.stop_requested.load(Ordering::SeqCst)
        }

        /// Tear the tap down and wake the run loop, exactly once.
        ///
        /// Idempotent by the `swap` on `stop_requested`: a second call returns
        /// immediately. Each handle is swapped to null *before* being touched,
        /// so this and `Drop for EventTapResources` can run concurrently
        /// without either double-releasing.
        fn request_stop(&self) {
            if self.stop_requested.swap(true, Ordering::SeqCst) {
                return;
            }

            // Swap each pointer to null BEFORE invalidating. The swap is the
            // ownership transfer: whoever gets a non-null value from swap is
            // responsible for teardown. This prevents the double-invalidate
            // race with `Drop for EventTapResources`.
            let tap = self.tap.swap(ptr::null_mut(), Ordering::SeqCst) as CFMachPortRef;
            if !tap.is_null() {
                unsafe {
                    CGEventTapEnable(tap, false);
                    CFMachPortInvalidate(tap);
                    CFRelease(tap as *const c_void);
                }
            }

            let source = self.source.swap(ptr::null_mut(), Ordering::SeqCst) as CFRunLoopSourceRef;
            if !source.is_null() {
                unsafe {
                    CFRunLoopSourceInvalidate(source);
                    CFRelease(source as *const c_void);
                }
            }

            // run_loop is NOT owned (CFRunLoopGetCurrent doesn't retain) — no CFRelease.
            let run_loop = self.run_loop.swap(ptr::null_mut(), Ordering::SeqCst) as CFRunLoopRef;
            if !run_loop.is_null() {
                unsafe {
                    CFRunLoopStop(run_loop);
                    CFRunLoopWakeUp(run_loop);
                }
            }
        }
    }

    /// Owner of everything the run loop thread allocates.
    ///
    /// Lives on the worker thread's stack, so unwinding out of `run_event_tap`
    /// — for any reason, including an early permission error — releases the tap
    /// through `Drop` without a separate cleanup path.
    struct EventTapResources {
        state: Box<HotkeyState>,
        tap: Option<CFMachPortRef>,
        source: Option<CFRunLoopSourceRef>,
        run_loop: Option<CFRunLoopRef>,
        control: Arc<RuntimeControl>,
    }

    impl EventTapResources {
        /// Allocate the callback state; the handles are filled in as the tap is
        /// built.
        fn new(tx: Sender<HotkeyEvent>, control: Arc<RuntimeControl>) -> Self {
            Self {
                state: Box::new(HotkeyState::new(tx, Arc::clone(&control))),
                tap: None,
                source: None,
                run_loop: None,
                control,
            }
        }

        /// Stable pointer to the boxed [`HotkeyState`], handed to
        /// `CGEventTapCreate` as `user_info`.
        fn user_info_ptr(&mut self) -> *mut c_void {
            (&mut *self.state as *mut HotkeyState).cast::<c_void>()
        }

        /// Record the tap port here and publish it to [`RuntimeControl`] so the
        /// callback can re-arm it and teardown can claim it.
        fn set_tap(&mut self, tap: CFMachPortRef) {
            self.tap = Some(tap);
            self.control
                .tap
                .store(tap.cast::<c_void>(), Ordering::SeqCst);
        }

        /// Record and publish the run loop source.
        fn set_source(&mut self, source: CFRunLoopSourceRef) {
            self.source = Some(source);
            self.control
                .source
                .store(source.cast::<c_void>(), Ordering::SeqCst);
        }

        /// Record and publish the worker's run loop, so a stop request from
        /// another thread can wake it.
        fn set_run_loop(&mut self, run_loop: CFRunLoopRef) {
            self.run_loop = Some(run_loop);
            self.control
                .run_loop
                .store(run_loop.cast::<c_void>(), Ordering::SeqCst);
        }
    }

    impl Drop for EventTapResources {
        fn drop(&mut self) {
            // Use atomic swap to claim ownership of each resource. If
            // `request_stop()` already swapped a pointer to null, we get null
            // and skip teardown for that resource (it was already cleaned up).
            // This eliminates the double-invalidate crash (EXC_BREAKPOINT in
            // CFRunLoopSourceInvalidate).

            let tap = self.control.tap.swap(ptr::null_mut(), Ordering::SeqCst) as CFMachPortRef;
            if !tap.is_null() {
                unsafe {
                    CGEventTapEnable(tap, false);
                    CFMachPortInvalidate(tap);
                    CFRelease(tap as *const c_void);
                }
            }

            let source =
                self.control.source.swap(ptr::null_mut(), Ordering::SeqCst) as CFRunLoopSourceRef;
            if !source.is_null() {
                unsafe {
                    CFRunLoopSourceInvalidate(source);
                    CFRelease(source as *const c_void);
                }
            }

            // run_loop is NOT owned (CFRunLoopGetCurrent doesn't retain) — no CFRelease.
            let run_loop = self
                .control
                .run_loop
                .swap(ptr::null_mut(), Ordering::SeqCst) as CFRunLoopRef;
            if !run_loop.is_null() {
                unsafe {
                    CFRunLoopStop(run_loop);
                    CFRunLoopWakeUp(run_loop);
                }
            }

            // Clear Option fields so they don't dangle.
            self.tap = None;
            self.source = None;
            self.run_loop = None;
        }
    }

    /// Owning handle for a live hotkey listener.
    ///
    /// Dropping it shuts the listener down, so the caller only has to keep it
    /// alive for as long as hotkeys should work. [`Self::shutdown`] is the
    /// explicit form and is idempotent.
    pub struct HotkeyRuntime {
        control: Arc<RuntimeControl>,
        worker: Option<JoinHandle<()>>,
        running_guard: Option<RunningGuard>,
    }

    impl HotkeyRuntime {
        /// Take ownership of a spawned worker and its listener slot.
        fn new(
            control: Arc<RuntimeControl>,
            worker: JoinHandle<()>,
            running_guard: RunningGuard,
        ) -> Self {
            Self {
                control,
                worker: Some(worker),
                running_guard: Some(running_guard),
            }
        }

        /// Stop the listener and join its thread.
        ///
        /// Idempotent — `Drop` calls it too, and a second call after an
        /// explicit shutdown returns immediately. A panicking worker is logged
        /// rather than propagated, because failing to join must not stop the
        /// listener slot from being released.
        pub fn shutdown(&mut self) {
            if self.worker.is_none() && self.running_guard.is_none() {
                return;
            }

            self.control.request_stop();
            if let Some(worker) = self.worker.take()
                && worker.join().is_err()
            {
                tracing::warn!("Hotkey worker thread panicked during shutdown");
            }
            self.running_guard.take();
        }
    }

    impl Drop for HotkeyRuntime {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    /// Decode a CoreGraphics modifier bitfield into the detector's snapshot
    /// type.
    fn modifiers_from_flags(flags: CGEventFlags) -> HotkeyModifierSnapshot {
        HotkeyModifierSnapshot {
            ctrl: (flags & K_CG_EVENT_FLAG_MASK_CONTROL) != 0,
            shift: (flags & K_CG_EVENT_FLAG_MASK_SHIFT) != 0,
            option: (flags & K_CG_EVENT_FLAG_MASK_ALTERNATE) != 0,
            cmd: (flags & K_CG_EVENT_FLAG_MASK_COMMAND) != 0,
            fn_key: (flags & K_CG_EVENT_FLAG_MASK_SECONDARY_FN) != 0,
        }
    }

    /// Returns true if the CGEventType signals that macOS forcibly disabled
    /// the tap (timeout from a slow callback, or user input). Pure logic — no
    /// FFI — so it is unit-testable without a live tap or Accessibility perms.
    fn is_tap_disabled_event(event_type: CGEventType) -> bool {
        matches!(
            event_type,
            K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT | K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT
        )
    }

    /// Map a macOS virtual keycode onto the keys the detector distinguishes;
    /// everything else becomes `Other`.
    fn map_keycode(keycode: i64) -> HotkeyPhysicalKey {
        match keycode {
            K_VK_OPTION => HotkeyPhysicalKey::LeftOption,
            K_VK_RIGHT_OPTION => HotkeyPhysicalKey::RightOption,
            K_VK_CONTROL => HotkeyPhysicalKey::LeftControl,
            K_VK_RIGHT_CONTROL => HotkeyPhysicalKey::RightControl,
            K_VK_FUNCTION => HotkeyPhysicalKey::Fn,
            K_VK_SPACE => HotkeyPhysicalKey::Space,
            K_VK_V => HotkeyPhysicalKey::V,
            _ => HotkeyPhysicalKey::Other,
        }
    }

    /// CGEventTap callback - thin adapter from CoreGraphics events to HotkeyDetector input.
    ///
    /// Note: the tap is created with `K_CG_EVENT_TAP_OPTION_LISTEN_ONLY`
    /// (see `run_event_tap`), so CoreGraphics ignores our return value and
    /// we cannot suppress events here. If real Fn-emoji-picker suppression
    /// is ever needed, the tap shape must change to an active tap first.
    extern "C" fn event_callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        // Skip processing if hotkeys are disabled
        if !ENABLED.load(Ordering::Relaxed) {
            return event;
        }

        let state_ptr = user_info.cast::<HotkeyState>();
        if state_ptr.is_null() {
            return event;
        }
        let state = unsafe { &mut *state_ptr };

        // macOS may forcibly disable a listen-only tap when our callback is too
        // slow (timeout) or on user input. Without re-arming here, every hotkey
        // (dictation/formatting/assistive) goes silently dead until restart.
        // Re-enable the tap immediately and warn. We only *read* the live tap
        // pointer from `control.tap`; we never invalidate or free it, so the
        // ownership/teardown contract (swap-to-null in `request_stop`/`Drop`)
        // is preserved — after stop the pointer is null and we no-op.
        if is_tap_disabled_event(event_type) {
            let reason = if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
                "timeout (slow callback)"
            } else {
                "user input"
            };
            let tap = state.control.tap.load(Ordering::SeqCst) as CFMachPortRef;
            if tap.is_null() {
                tracing::warn!(
                    "CGEventTap disabled by {reason} but tap port is null (shutting down); skipping re-arm"
                );
            } else {
                unsafe {
                    // SAFETY: `tap` is loaded from `RuntimeControl.tap` after a null check.
                    // We only ask CoreGraphics to re-enable the live event tap; ownership,
                    // invalidation, and release remain with the runtime control teardown path.
                    CGEventTapEnable(tap, true);
                }
                tracing::warn!(
                    "CGEventTap disabled by {reason}; re-armed tap to keep hotkeys alive"
                );
            }
            return event;
        }

        // SAFETY: `event` is the CGEventRef CoreGraphics passes to this tap
        // callback; it is valid for the duration of the callback and these
        // calls are read-only accessors that take no ownership.
        let flags = unsafe { CGEventGetFlags(event) };
        let modifiers = modifiers_from_flags(flags);
        let now = Instant::now();
        let runtime_config = get_hotkey_runtime_config();

        let input = match event_type {
            K_CG_EVENT_KEY_DOWN => {
                // SAFETY: read-only field accessor on the callback-owned
                // `event`; valid for the callback's duration (see above).
                let keycode =
                    unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
                HotkeyDetectorInput::KeyDown {
                    now,
                    key: map_keycode(keycode),
                    modifiers,
                }
            }
            K_CG_EVENT_KEY_UP => {
                // SAFETY: read-only field accessor on the callback-owned
                // `event`; valid for the callback's duration (see above).
                let keycode =
                    unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
                HotkeyDetectorInput::KeyUp {
                    key: map_keycode(keycode),
                    modifiers,
                }
            }
            K_CG_EVENT_FLAGS_CHANGED => {
                let keycode =
                    unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
                let key = map_keycode(keycode);

                tracing::debug!(
                    "CGEventTap: flags=0x{:X} keycode={} (ctrl={}, shift={}, opt={}, cmd={}, fn={})",
                    flags,
                    keycode,
                    modifiers.ctrl,
                    modifiers.shift,
                    modifiers.option,
                    modifiers.cmd,
                    modifiers.fn_key
                );

                HotkeyDetectorInput::FlagsChanged {
                    now,
                    key,
                    modifiers,
                }
            }
            _ => return event,
        };

        if let Some(hotkey_event) = state.detector.feed(input, runtime_config) {
            let _ = state.tx.send(hotkey_event);
        }

        event
    }
    /// Start the hotkey listener on a background thread and return its runtime owner.
    pub fn start_listener(tx: Sender<HotkeyEvent>) -> Result<HotkeyRuntime, String> {
        let running_guard = RunningGuard::acquire()?;
        let control = Arc::new(RuntimeControl::default());
        let worker_control = Arc::clone(&control);

        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let worker = thread::spawn(move || {
            if let Err(e) = run_event_tap(tx, worker_control, ready_tx) {
                tracing::error!("CGEventTap error: {}", e);
            }
        });

        let mut runtime = HotkeyRuntime::new(control, worker, running_guard);

        // Wait for startup confirmation so we can surface permission errors.
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(runtime),
            Ok(Err(e)) => {
                runtime.shutdown();
                Err(e)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                runtime.shutdown();
                Err(
                    "Timed out while starting CGEventTap (hotkeys). Check Accessibility permission."
                        .to_string(),
                )
            }
            Err(e) => {
                runtime.shutdown();
                Err(format!("Failed to start hotkeys: {}", e))
            }
        }
    }

    /// Enable hotkey processing (thread-safe)
    pub fn enable() {
        ENABLED.store(true, Ordering::SeqCst);
        tracing::info!("Hotkeys enabled");
    }

    /// Disable hotkey processing (thread-safe)
    pub fn disable() {
        ENABLED.store(false, Ordering::SeqCst);
        tracing::info!("Hotkeys disabled");
    }

    /// Check if hotkeys are currently enabled (thread-safe)
    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::SeqCst)
    }

    /// Run the CGEventTap on the current thread (blocking)
    fn run_event_tap(
        tx: Sender<HotkeyEvent>,
        control: Arc<RuntimeControl>,
        ready_tx: mpsc::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let mut resources = EventTapResources::new(tx, control);

        // Key-up resets one-shot command chords so key repeat cannot emit duplicates.
        let event_mask: u64 =
            (1 << K_CG_EVENT_FLAGS_CHANGED) | (1 << K_CG_EVENT_KEY_DOWN) | (1 << K_CG_EVENT_KEY_UP);

        // Create the event tap
        let tap = unsafe {
            CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
                event_mask,
                event_callback,
                resources.user_info_ptr(),
            )
        };

        if tap.is_null() {
            let msg = "Failed to create CGEventTap - check Accessibility permission".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        resources.set_tap(tap);

        // Enable the tap
        unsafe {
            CGEventTapEnable(tap, true);
        }

        // Verify tap is actually enabled
        let is_enabled = unsafe { CGEventTapIsEnabled(tap) };
        if !is_enabled {
            tracing::error!("CGEventTap failed to enable! macOS may have denied it.");
            let msg = "CGEventTap not enabled - macOS denied access".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        tracing::debug!("CGEventTap verified as enabled");

        // Create run loop source
        let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };

        if source.is_null() {
            let msg = "Failed to create run loop source".to_string();
            let _ = ready_tx.send(Err(msg.clone()));
            return Err(msg);
        }
        resources.set_source(source);

        // Add to run loop
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        resources.set_run_loop(run_loop);
        unsafe {
            CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        }

        let bindings = get_mode_hotkey_bindings();
        tracing::info!(
            "CGEventTap started with mode bindings: dictation={:?}, formatting={:?}, assistive={:?}",
            bindings.dictation,
            bindings.formatting,
            bindings.assistive
        );
        let _ = ready_tx.send(Ok(()));

        // Run until an explicit shutdown request stops this run loop.
        tracing::debug!("Entering CFRunLoopRun (blocks until stop)");
        if resources.control.is_stop_requested() {
            unsafe {
                CFRunLoopStop(run_loop);
                CFRunLoopWakeUp(run_loop);
            }
        } else {
            unsafe {
                CFRunLoopRun();
            }
        }

        tracing::info!("CGEventTap run loop exited");

        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::Mutex;

        static LIFECYCLE_TEST_LOCK: Mutex<()> = Mutex::new(());

        fn spawn_test_runtime() -> HotkeyRuntime {
            let running_guard = RunningGuard::acquire().expect("test runtime should acquire guard");
            let control = Arc::new(RuntimeControl::default());
            let worker_control = Arc::clone(&control);
            let worker = thread::spawn(move || {
                while !worker_control.is_stop_requested() {
                    thread::sleep(Duration::from_millis(5));
                }
            });
            HotkeyRuntime::new(control, worker, running_guard)
        }

        #[test]
        fn is_tap_disabled_event_detects_disabled_sentinels() {
            assert!(is_tap_disabled_event(0xFFFF_FFFE));
            assert!(is_tap_disabled_event(0xFFFF_FFFF));
            assert!(is_tap_disabled_event(K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT));
            assert!(is_tap_disabled_event(K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT));

            assert!(!is_tap_disabled_event(K_CG_EVENT_KEY_DOWN));
            assert!(!is_tap_disabled_event(K_CG_EVENT_KEY_UP));
            assert!(!is_tap_disabled_event(K_CG_EVENT_FLAGS_CHANGED));
            assert!(!is_tap_disabled_event(10));
            assert!(!is_tap_disabled_event(12));
            assert!(!is_tap_disabled_event(0));
        }

        #[test]
        fn running_guard_blocks_double_start() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            let first = RunningGuard::acquire().expect("first start must succeed");
            assert!(RunningGuard::acquire().is_err());
            drop(first);

            let second = RunningGuard::acquire().expect("second start after drop must succeed");
            drop(second);
            assert!(!RUNNING.load(Ordering::SeqCst));
        }

        #[test]
        fn runtime_shutdown_is_idempotent() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            let mut runtime = spawn_test_runtime();
            runtime.shutdown();
            runtime.shutdown();

            assert!(!RUNNING.load(Ordering::SeqCst));
        }

        #[test]
        fn runtime_drop_stops_worker_without_panic() {
            let _guard = LIFECYCLE_TEST_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            RUNNING.store(false, Ordering::SeqCst);

            {
                let _runtime = spawn_test_runtime();
                assert!(RUNNING.load(Ordering::SeqCst));
            }

            assert!(!RUNNING.load(Ordering::SeqCst));
        }
    }
}

// --- Fallback for non-macOS ---

/// Stub with the same name and surface as the macOS module, so the rest of the
/// app compiles unchanged off-platform.
///
/// Every entry point warns and succeeds rather than failing: a machine without
/// an event tap should still run the app, just without hotkeys.
#[cfg(not(target_os = "macos"))]
mod macos {
    use super::*;

    /// Inert stand-in for the macOS runtime handle.
    pub struct HotkeyRuntime;

    impl HotkeyRuntime {
        /// No-op — there is nothing to tear down.
        pub fn shutdown(&mut self) {}
    }

    /// Warn and hand back an inert runtime; never an error, so startup is not
    /// blocked by the absence of hotkey support.
    pub fn start_listener(_tx: Sender<HotkeyEvent>) -> Result<HotkeyRuntime, String> {
        tracing::warn!("Hotkey listener not supported on this platform");
        Ok(HotkeyRuntime)
    }

    /// No-op — warns that hotkeys do not exist here.
    pub fn enable() {
        tracing::warn!("Hotkey enable not supported on this platform");
    }

    /// No-op — warns that hotkeys do not exist here.
    pub fn disable() {
        tracing::warn!("Hotkey disable not supported on this platform");
    }

    /// Always `false` off macOS.
    pub fn is_enabled() -> bool {
        false
    }
}

pub use macos::{HotkeyRuntime, disable, enable, is_enabled, start_listener};
