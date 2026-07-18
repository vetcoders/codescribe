use super::config::{get_hotkey_runtime_config, get_mode_hotkey_bindings};
use super::detector::{
    HotkeyDetector, HotkeyDetectorInput, HotkeyEvent, HotkeyModifierSnapshot, HotkeyPhysicalKey,
};
use crossbeam_channel::Sender;
use std::time::{Duration, Instant};

// --- macOS CGEventTap Implementation using raw bindings ---

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
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *mut c_void;
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;

    type CGEventType = u32;
    type CGEventFlags = u64;
    type CGEventField = u32;

    // CGEventType values
    const K_CG_EVENT_KEY_DOWN: CGEventType = 10;
    const K_CG_EVENT_KEY_UP: CGEventType = 11;
    const K_CG_EVENT_FLAGS_CHANGED: CGEventType = 12;

    // CGEventType "tap disabled" sentinels. CoreGraphics emits these (the two
    // highest u32 values) when it forcibly disables a tap — either because a
    // listen-only callback was too slow or because of user input during a
    // sensitive sequence. They live in <CoreGraphics/CGEvent.h> as stable ABI
    // constants, named there `kCGEventTapDisabledByTimeout` and
    // `kCGEventTapDisabledByUserInput`.
    const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: CGEventType = 0xFFFF_FFFE;
    const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: CGEventType = 0xFFFF_FFFF;

    // CGEventFlags masks
    const K_CG_EVENT_FLAG_MASK_CONTROL: CGEventFlags = 0x00040000;
    const K_CG_EVENT_FLAG_MASK_SHIFT: CGEventFlags = 0x00020000;
    const K_CG_EVENT_FLAG_MASK_ALTERNATE: CGEventFlags = 0x00080000; // Option key
    const K_CG_EVENT_FLAG_MASK_COMMAND: CGEventFlags = 0x00100000;
    const K_CG_EVENT_FLAG_MASK_SECONDARY_FN: CGEventFlags = 0x00800000;

    // CGEventField for keycode
    const K_CG_KEYBOARD_EVENT_KEYCODE: CGEventField = 9;

    // macOS virtual keycodes for Option keys
    const K_VK_OPTION: i64 = 58; // Left Option
    const K_VK_RIGHT_OPTION: i64 = 61; // Right Option
    // macOS virtual keycodes for Control keys
    const K_VK_CONTROL: i64 = 59; // Left Control
    const K_VK_RIGHT_CONTROL: i64 = 62; // Right Control
    const K_VK_FUNCTION: i64 = 63; // Fn (Globe)
    const K_VK_SPACE: i64 = 49;

    // CGEventTap constants
    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;

    // Callback type
    type CGEventTapCallBack = extern "C" fn(
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
        fn CGEventGetFlags(event: CGEventRef) -> CGEventFlags;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFMachPortCreateRunLoopSource(
            allocator: *const c_void,
            port: CFMachPortRef,
            order: i64,
        ) -> CFRunLoopSourceRef;
        fn CFMachPortInvalidate(port: CFMachPortRef);

        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
        fn CFRunLoopSourceInvalidate(source: CFRunLoopSourceRef);
        fn CFRunLoopRun();
        fn CFRunLoopStop(rl: CFRunLoopRef);
        fn CFRunLoopWakeUp(rl: CFRunLoopRef);
        fn CFRelease(cf: *const c_void);

        static kCFRunLoopCommonModes: *const c_void;
    }

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
        fn new(tx: Sender<HotkeyEvent>, control: Arc<RuntimeControl>) -> Self {
            Self {
                detector: HotkeyDetector::default(),
                tx,
                control,
            }
        }
    }

    static RUNNING: AtomicBool = AtomicBool::new(false);
    static ENABLED: AtomicBool = AtomicBool::new(true);

    struct RunningGuard;

    impl RunningGuard {
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

    #[derive(Default)]
    struct RuntimeControl {
        stop_requested: AtomicBool,
        tap: AtomicPtr<c_void>,
        source: AtomicPtr<c_void>,
        run_loop: AtomicPtr<c_void>,
    }

    impl RuntimeControl {
        fn is_stop_requested(&self) -> bool {
            self.stop_requested.load(Ordering::SeqCst)
        }

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

    struct EventTapResources {
        state: Box<HotkeyState>,
        tap: Option<CFMachPortRef>,
        source: Option<CFRunLoopSourceRef>,
        run_loop: Option<CFRunLoopRef>,
        control: Arc<RuntimeControl>,
    }

    impl EventTapResources {
        fn new(tx: Sender<HotkeyEvent>, control: Arc<RuntimeControl>) -> Self {
            Self {
                state: Box::new(HotkeyState::new(tx, Arc::clone(&control))),
                tap: None,
                source: None,
                run_loop: None,
                control,
            }
        }

        fn user_info_ptr(&mut self) -> *mut c_void {
            (&mut *self.state as *mut HotkeyState).cast::<c_void>()
        }

        fn set_tap(&mut self, tap: CFMachPortRef) {
            self.tap = Some(tap);
            self.control
                .tap
                .store(tap.cast::<c_void>(), Ordering::SeqCst);
        }

        fn set_source(&mut self, source: CFRunLoopSourceRef) {
            self.source = Some(source);
            self.control
                .source
                .store(source.cast::<c_void>(), Ordering::SeqCst);
        }

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

    pub struct HotkeyRuntime {
        control: Arc<RuntimeControl>,
        worker: Option<JoinHandle<()>>,
        running_guard: Option<RunningGuard>,
    }

    impl HotkeyRuntime {
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

    fn map_keycode(keycode: i64) -> HotkeyPhysicalKey {
        match keycode {
            K_VK_OPTION => HotkeyPhysicalKey::LeftOption,
            K_VK_RIGHT_OPTION => HotkeyPhysicalKey::RightOption,
            K_VK_CONTROL => HotkeyPhysicalKey::LeftControl,
            K_VK_RIGHT_CONTROL => HotkeyPhysicalKey::RightControl,
            K_VK_FUNCTION => HotkeyPhysicalKey::Fn,
            K_VK_SPACE => HotkeyPhysicalKey::Space,
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

        let flags = unsafe { CGEventGetFlags(event) };
        let modifiers = modifiers_from_flags(flags);
        let now = Instant::now();
        let runtime_config = get_hotkey_runtime_config();

        let input = match event_type {
            K_CG_EVENT_KEY_DOWN => {
                let keycode =
                    unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
                HotkeyDetectorInput::KeyDown {
                    now,
                    key: map_keycode(keycode),
                    modifiers,
                }
            }
            K_CG_EVENT_KEY_UP => {
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

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::*;

    pub struct HotkeyRuntime;

    impl HotkeyRuntime {
        pub fn shutdown(&mut self) {}
    }

    pub fn start_listener(_tx: Sender<HotkeyEvent>) -> Result<HotkeyRuntime, String> {
        tracing::warn!("Hotkey listener not supported on this platform");
        Ok(HotkeyRuntime)
    }

    pub fn enable() {
        tracing::warn!("Hotkey enable not supported on this platform");
    }

    pub fn disable() {
        tracing::warn!("Hotkey disable not supported on this platform");
    }

    pub fn is_enabled() -> bool {
        false
    }
}

pub use macos::{HotkeyRuntime, disable, enable, is_enabled, start_listener};
