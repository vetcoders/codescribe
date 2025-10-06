# ui.py
#
# purpose: manages user interface elements, specifically the macos menu bar
#          icon updates and the functionality to paste text into the active application.
#
# dependencies: appkit (pyobjc wrapper for cocoa ui elements like nspasteboard)
#               quartz (pyobjc wrapper for coregraphics event simulation like key presses)
#               time (for small delays in paste simulation)
#               logging (for status messages)
#
# key components: menuicon class (static methods to update app title/icon)
#                 paste_text function (copies text to clipboard, simulates cmd+v)
#
# design rationale: uses appkit (via rumps integration indirectly for icon, directly
#                   for pasteboard) and quartz for native macos integration. icon
#                   updates provide visual feedback on the app's state. paste_text
#                   directly manipulates the general pasteboard and then simulates
#                   command-v keystrokes using coregraphics events for seamless pasting.
#                   the timer in menuicon.success provides a brief visual confirmation
#                   before resetting the icon.
#
try:
    import AppKit
    import Quartz
    from Foundation import NSData, NSOperationQueue, NSThread
except Exception:  # Allow importing ui helpers in non-macOS test envs
    AppKit = None  # type: ignore
    Quartz = None  # type: ignore
    NSOperationQueue = None  # type: ignore
    NSThread = None  # type: ignore
    NSData = None  # type: ignore
import logging
import os
import time

# configure logging
logging.basicConfig(level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s")

# --- constants ---
ICON_IDLE = "🜏"  # u+1f70f (alchemical symbol for distillation)
ICON_LISTEN = "◉"  # u+25c9 (fisheye)
ICON_THINK = "…"  # u+2026 (horizontal ellipsis)
ICON_SUCCESS = "✓"  # u+2713 (check mark)

# --- menu icon management ---


class MenuIcon:
    """provides static methods to manage the app's menu bar icon state.

    uses specific unicode glyphs to represent different application states.
    interacts with the rumps app object to change its title (which is the icon).
    includes a timer mechanism to briefly show success before resetting to idle.
    """

    # Glyph constants (avoid name clash with methods)
    IDLE = ICON_IDLE
    LISTEN = ICON_LISTEN
    THINK = ICON_THINK
    SUCCESS = ICON_SUCCESS

    @staticmethod
    def _set_impl(app, glyph: str):
        """sets the menu bar icon to the specified glyph.

        if an image icon is present (app.icon), keep the title empty to avoid
        overlaying text next to the tray image.

        args:
            app (rumps.app): the main application instance.
            glyph (str): the unicode character to use as the icon.
        """
        if app:
            if getattr(app, "icon", None):
                # When an image icon is used, keep title empty
                app.title = ""
            else:
                app.title = glyph
        else:
            logging.warning("Attempted to set menu icon, but app object was None.")

    @staticmethod
    def set(app, glyph: str):
        """Thread-safe wrapper to update tray title on the main thread."""
        if NSThread is not None and not NSThread.isMainThread():
            if NSOperationQueue is not None:
                NSOperationQueue.mainQueue().addOperationWithBlock_(
                    lambda: MenuIcon._set_impl(app, glyph)
                )
                return
        MenuIcon._set_impl(app, glyph)

    @staticmethod
    def listen(app):
        """sets the icon to the 'listening' state.

        args:
            app (rumps.app): the main application instance.
        """
        MenuIcon.set(app, ICON_LISTEN)
        logging.info(f"UI State: Listening ({ICON_LISTEN})")

    @staticmethod
    def think(app):
        """sets the icon to the 'thinking/processing' state.

        args:
            app (rumps.app): the main application instance.
        """
        MenuIcon.set(app, ICON_THINK)
        logging.info(f"UI State: Thinking ({ICON_THINK})")

    @staticmethod
    def success(app):
        """sets the icon to the 'success' state and schedules reset to idle.

        args:
            app (rumps.app): the main application instance.
        """

        def _do_success():
            MenuIcon._set_impl(app, ICON_SUCCESS)
            logging.info(f"UI State: Success ({ICON_SUCCESS})")
            if AppKit is not None:
                AppKit.NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                    1.0,  # interval (seconds)
                    app,  # target object (the rumps app instance)
                    "reset:",  # selector (method name to call - note the colon for objc)
                    None,  # userinfo (optional data)
                    False,  # repeats (no)
                )
                logging.info(f"Scheduled UI reset to Idle ({ICON_IDLE}) in 1 second.")
            else:
                logging.debug("AppKit not available; skipping NSTimer scheduling in tests.")

        # Ensure we schedule on the main thread
        if NSThread is not None and not NSThread.isMainThread() and NSOperationQueue is not None:
            NSOperationQueue.mainQueue().addOperationWithBlock_(_do_success)
        else:
            _do_success()


def backend_status_labels(stt_ok: bool, llm_ok: bool) -> list[str]:
    """Return two labels indicating backend connectivity for tray menu.

    This helper is pure and easy to unit-test without macOS frameworks.
    """
    stt_label = f"STT: {'OK' if stt_ok else 'OFF'}"
    llm_label = f"LLM: {'OK' if llm_ok else 'OFF'}"
    return [stt_label, llm_label]


# --- config helpers (pure) ---
try:
    # Only used for typing; keep import optional
    from config import Config  # type: ignore
except Exception:  # pragma: no cover
    Config = None  # type: ignore


def config_labels(cfg) -> list[str]:
    """Return user-facing labels for config to show in tray menu.

    Accepts a config.Config-like object with attributes: language, format_enabled,
    whisper_url, llm_url. Kept untyped to avoid import errors in test envs.
    """
    lang = getattr(cfg, "language", None) or "auto"
    fmt = "ON" if getattr(cfg, "format_enabled", False) else "OFF"
    wurl = getattr(cfg, "whisper_url", "") or "local"
    lurl = getattr(cfg, "llm_url", "") or "local"
    return [
        f"Language: {lang}",
        f"Formatting: {fmt}",
        f"Whisper URL: {wurl}",
        f"LLM URL: {lurl}",
    ]


def toggles_help_message(lang: str = "en") -> str:
    """Return the help message for the tray toggles.

    Currently provides English text by default to be accessible to non-Polish speakers.
    """
    # For now only English is provided; can be extended later.
    return (
        "Language (Auto/PL/EN):\n"
        "- Auto: Whisper automatically detects the language.\n"
        "- PL/EN: Forces the transcription language.\n\n"
        "Enable Formatting:\n"
        "- Off (default): Pastes the raw Whisper output.\n"
        "- On: Cleans and polishes the text using a local LLM (e.g., Bielik)."
    )

    # note: the actual reset_ method needs to be defined within the rumps.app subclass in main.py
    #       because nstimer calls the selector on the *target* object.
    #       we keep this comment here for clarity.
    # @staticmethod
    # def reset_(app, _timer):
    #     """(this method belongs in the rumps.app class)
    #     resets the icon to the 'idle' state.
    #     called by the nstimer scheduled in success().
    #
    #     args:
    #         app (rumps.app): the main application instance (passed as self).
    #         _timer (nstimer): the timer object (unused).
    #     """
    #     menuicon.set(app, menuicon.idle)
    #     logging.info("ui state: idle (🜏)")


# --- clipboard and paste ---


def paste_text(text: str):
    """pastes the given text into the currently active application field.

    it first clears the system pasteboard, copies the new text to it,
    and then simulates a command+v keypress event.

    requires accessibility permissions for simulating keystrokes.

    args:
        text (str): the text to be pasted.
    """
    if not text:
        logging.warning("Paste called with empty text.")
        return

    logging.info(f"Pasting text: '{text[:50]}...' ({len(text)} chars)")

    def _snapshot_pasteboard(pb):
        """Capture full pasteboard contents (all items/types) and changeCount.

        Returns a tuple (snapshot, change_count) where snapshot is a list of
        items, each item being a list of (uti: str, data: bytes).
        """
        snapshot: list[list[tuple[str, bytes]]] = []
        try:
            items = pb.pasteboardItems() or []
            for it in items:
                types = list(it.types() or [])
                entry: list[tuple[str, bytes]] = []
                for uti in types:
                    try:
                        nsdata = it.dataForType_(uti)
                        if nsdata is not None:
                            entry.append((str(uti), bytes(nsdata)))
                    except Exception:
                        # ignore types we can't read
                        pass
                snapshot.append(entry)
        except Exception:
            # Fallback: capture plain string
            try:
                s = pb.stringForType_(AppKit.NSStringPboardType)
                if s is not None:
                    bs = s.encode("utf-8", "surrogatepass")
                    snapshot = [[("public.utf8-plain-text", bs)]]
            except Exception:
                pass
        return snapshot, int(pb.changeCount())

    def _restore_pasteboard_if_unchanged(pb, snapshot, our_count):
        try:
            # Only restore if no one changed the clipboard since our write
            if int(pb.changeCount()) != int(our_count):
                return
            pb.clearContents()
            wrote_any = False
            for entry in snapshot:
                item = AppKit.NSPasteboardItem.alloc().init()
                ok_all = True
                for uti, data in entry:
                    try:
                        nsdata = NSData.dataWithBytes_length_(data, len(data))
                        ok = item.setData_forType_(nsdata, uti)
                        ok_all = ok_all and bool(ok)
                    except Exception:
                        ok_all = False
                if ok_all:
                    pb.writeObjects_([item])
                    wrote_any = True
            if wrote_any:
                logging.info("Clipboard restored to previous contents.")
        except Exception:
            logging.debug("Clipboard restore skipped due to error.")

    def _do_paste():
        try:
            # 1. copy text to pasteboard
            pasteboard = AppKit.NSPasteboard.generalPasteboard()
            # Snapshot current clipboard (for optional restore)
            restore_enabled = os.environ.get("RESTORE_CLIPBOARD", "1").lower() not in (
                "0",
                "false",
                "no",
                "off",
            )
            snapshot = None
            before_count = None
            if restore_enabled:
                snapshot, before_count = _snapshot_pasteboard(pasteboard)

            pasteboard.clearContents()  # clear existing contents
            pasteboard.declareTypes_owner_([AppKit.NSStringPboardType], None)
            success = pasteboard.setString_forType_(text, AppKit.NSStringPboardType)
            if not success:
                logging.error("Failed to set string on pasteboard.")
                return
            logging.info("Text successfully copied to clipboard.")
            our_count = int(pasteboard.changeCount())

            # 2. simulate cmd+v keypress
            source = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStateCombinedSessionState)
            if not source:
                logging.error("Failed to create CGEventSource.")
                return

            v_keycode = 9  # key code for 'v'
            event_down = Quartz.CGEventCreateKeyboardEvent(source, v_keycode, True)
            if not event_down:
                logging.error("Failed to create key down event.")
                return
            Quartz.CGEventSetFlags(event_down, Quartz.kCGEventFlagMaskCommand)

            event_up = Quartz.CGEventCreateKeyboardEvent(source, v_keycode, False)
            if not event_up:
                logging.error("Failed to create key up event.")
                return
            Quartz.CGEventSetFlags(event_up, Quartz.kCGEventFlagMaskCommand)

            Quartz.CGEventPost(Quartz.kCGHIDEventTap, event_down)
            time.sleep(0.01)
            Quartz.CGEventPost(Quartz.kCGHIDEventTap, event_up)

            logging.info("Command+V keypress simulated successfully.")

            # Optional: restore previous clipboard shortly after paste
            if restore_enabled and snapshot is not None:
                import threading

                delay_ms = int(os.environ.get("RESTORE_CLIPBOARD_DELAY_MS", "200"))

                def _delayed_restore():
                    time.sleep(max(0, delay_ms) / 1000.0)
                    if NSOperationQueue is not None:
                        NSOperationQueue.mainQueue().addOperationWithBlock_(
                            lambda: _restore_pasteboard_if_unchanged(
                                pasteboard, snapshot, our_count
                            )
                        )

                threading.Thread(target=_delayed_restore, daemon=True).start()

        except Exception as e:
            logging.error(f"Error during paste operation: {e}", exc_info=True)
            logging.error("Ensure Accessibility permissions are granted for the application.")

    # Ensure UI interaction happens on main thread (AppKit/Quartz are main-thread-only)
    if NSThread is not None and not NSThread.isMainThread() and NSOperationQueue is not None:
        NSOperationQueue.mainQueue().addOperationWithBlock_(_do_paste)
    else:
        _do_paste()


def start_sound():
    """Play a soft, non-error start sound.

    Tries macOS system sounds (NSSound) with low volume. The sound and volume
    can be customized via env vars: SOUND_NAME (e.g., 'Tink', 'Pop') and
    SOUND_VOLUME (0.0–1.0). Falls back to a quiet ASCII bell if AppKit is not
    available.
    """

    def _play():
        try:
            name = os.environ.get("SOUND_NAME", "Tink")
            volume = float(os.environ.get("SOUND_VOLUME", "0.2"))
            if AppKit is not None and hasattr(AppKit, "NSSound"):
                snd = AppKit.NSSound.soundNamed_(name)
                if snd is None:
                    snd = AppKit.NSSound.soundNamed_("Pop")
                if snd is not None:
                    try:
                        snd.setVolume_(max(0.0, min(1.0, volume)))
                    except Exception:
                        pass
                    snd.play()
                    return
            # TTY/headless fallback
            print("\a", end="", flush=True)
        except Exception:
            logging.debug("start_sound failed; continuing without sound")

    if NSThread is not None and not NSThread.isMainThread() and NSOperationQueue is not None:
        NSOperationQueue.mainQueue().addOperationWithBlock_(_play)
    else:
        _play()
