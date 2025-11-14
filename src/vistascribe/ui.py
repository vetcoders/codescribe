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

from .settings_store import get_settings

# configure logging
logging.basicConfig(level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s")

# --- constants ---
# Use simple, recognizable glyphs in the menu bar.
# Idle: bullet (•) to avoid the obscure alchemical symbol previously used.
# Allow overrides via env: TRAY_GLYPH_IDLE/LISTEN/THINK/SUCCESS
ICON_IDLE = os.environ.get("TRAY_GLYPH_IDLE", "•") or "•"  # U+2022 (bullet)
ICON_LISTEN = os.environ.get("TRAY_GLYPH_LISTEN", "◉") or "◉"  # U+25C9 (fisheye)
ICON_THINK = os.environ.get("TRAY_GLYPH_THINK", "…") or "…"  # U+2026 (horizontal ellipsis)
ICON_SUCCESS = os.environ.get("TRAY_GLYPH_SUCCESS", "✓") or "✓"  # U+2713 (check mark)

# Quartz key codes we rely on when simulating key presses
KEYCODE_RIGHT_ARROW = 124


def _tray_show_glyph() -> bool:
    val = (os.environ.get("SHOW_TRAY_GLYPH", "1") or "").strip().lower()
    return val not in {"0", "false", "no", "off"}


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
            dev_mode = os.environ.get("DEV_MODE", "0").lower() in ("1", "true", "yes", "on")
            show_title = dev_mode or _tray_show_glyph()
            if getattr(app, "icon", None) and not show_title:
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
    """Return user-facing labels for STT and AI formatter readiness."""

    stt_label = f"STT: {'OK' if stt_ok else 'OFF'}"
    settings = get_settings()
    if not settings.ai_formatting_enabled:
        llm_label = "AI: Light+ only"
    else:
        provider = "Harmony" if settings.ai_provider == "harmony" else "Ollama"
        status = "OK" if llm_ok else "OFF"
        llm_label = f"AI: {provider} ({status})"

    return [stt_label, llm_label]


# --- config helpers (pure) ---
try:
    # Only used for typing; keep import optional
    from .config import Config  # type: ignore
except Exception:  # pragma: no cover
    Config = None  # type: ignore


def config_labels(cfg) -> list[str]:
    """Return user-facing labels for config to show in tray menu.

    Accepts a config.Config-like object with attributes: language, format_enabled,
    whisper_url, llm_url. Kept untyped to avoid import errors in test envs.
    """
    lang = getattr(cfg, "language", None) or "auto"
    fmt_enabled = bool(getattr(cfg, "format_enabled", False))
    provider = getattr(cfg, "ai_provider", "harmony")
    fmt = "ON" if fmt_enabled else "OFF"
    wurl = getattr(cfg, "whisper_url", "") or "local"
    lurl = getattr(cfg, "llm_url", "") or "local"
    return [
        f"Language: {lang}",
        f"AI Formatting: {fmt} ({provider})",
        f"Whisper URL: {wurl}",
        f"Harmony URL: {lurl}",
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
        "AI Formatting:\n"
        "- Off (default): Light+ cleanup only.\n"
        "- On: Sends Light+ output to Harmony/Ollama for polishing."
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
                logging.debug("Clipboard was modified by user, skipping restore")
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

            # CRITICAL: Move cursor to end of pasted text (clear selection)
            # This prevents restored clipboard from REPLACING pasted text in auto-paste apps
            time.sleep(0.05)  # Let paste settle

            # Simulate Right Arrow key to deselect and move cursor to end
            arrow_down = Quartz.CGEventCreateKeyboardEvent(source, KEYCODE_RIGHT_ARROW, True)
            arrow_up = Quartz.CGEventCreateKeyboardEvent(source, KEYCODE_RIGHT_ARROW, False)
            if arrow_down and arrow_up:
                Quartz.CGEventPost(Quartz.kCGHIDEventTap, arrow_down)
                time.sleep(0.005)
                Quartz.CGEventPost(Quartz.kCGHIDEventTap, arrow_up)
                logging.debug("Cleared selection (moved cursor to end)")

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


def copy_text(text: str) -> bool:
    """Copy text to clipboard without simulating paste."""
    if not text:
        logging.warning("Copy called with empty text.")
        return False
    try:
        if AppKit is None:
            logging.warning("AppKit unavailable; cannot access pasteboard.")
            return False
        pasteboard = AppKit.NSPasteboard.generalPasteboard()
        pasteboard.clearContents()
        pasteboard.declareTypes_owner_([AppKit.NSStringPboardType], None)
        ok = pasteboard.setString_forType_(text, AppKit.NSStringPboardType)
        return bool(ok)
    except Exception as exc:
        logging.error(f"Failed to copy text: {exc}")
        return False


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


# --- recording indicator near cursor (for hold gesture) ---

_badge_window = None
_badge_timer = None
_badge_timer_target = None  # Objective-C target to drive timer ticks

_EDITABLE_ROLES = {
    getattr(AppKit, "NSAccessibilityTextAreaRole", "AXTextArea"),
    getattr(AppKit, "NSAccessibilityTextFieldRole", "AXTextField"),
    getattr(AppKit, "NSAccessibilitySearchFieldRole", "AXSearchField"),
    getattr(AppKit, "NSAccessibilityComboBoxRole", "AXComboBox"),
    getattr(AppKit, "NSAccessibilitySecureTextFieldRole", "AXSecureTextField"),
    "AXText",  # generic
    "AXParagraph",
}

_EDITABLE_SUBROLES = {
    getattr(AppKit, "NSAccessibilityStandardWindowSubrole", "AXStandardWindow"),
    getattr(AppKit, "NSAccessibilityTableRowSubrole", "AXTableRow"),
}


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, str(default)).strip())
    except Exception:
        return default


def _badge_size_px() -> int:
    # Allows tiny dot (e.g., 4) up to a small badge size
    v = max(2, min(32, _env_int("HOLD_BADGE_SIZE", 12)))
    return v


def _badge_offsets() -> tuple[float, float]:
    # Positive X moves right; positive Y moves up relative to cursor/caret origin
    ox = float(os.environ.get("HOLD_BADGE_OFFSET_X", "10").strip() or 10)
    oy = float(os.environ.get("HOLD_BADGE_OFFSET_Y", "-10").strip() or -10)
    return ox, oy


def hold_indicator_enabled() -> bool:
    return os.environ.get("HOLD_INDICATOR", "1").lower() not in ("0", "false", "no", "off")


def set_hold_indicator_enabled(flag: bool) -> None:
    os.environ["HOLD_INDICATOR"] = "1" if flag else "0"


def _ensure_badge_window():
    global _badge_window
    if _badge_window is not None or AppKit is None:
        return
    size = float(_badge_size_px())
    frame = ((0.0, 0.0), (size, size))
    style = AppKit.NSBorderlessWindowMask
    w = AppKit.NSWindow.alloc().initWithContentRect_styleMask_backing_defer_(
        frame, style, AppKit.NSBackingStoreBuffered, False
    )
    w.setOpaque_(False)
    w.setBackgroundColor_(AppKit.NSColor.clearColor())
    w.setLevel_(AppKit.NSStatusWindowLevel)
    try:
        w.setIgnoresMouseEvents_(True)
        # Keep visible across spaces and in full screen
        if hasattr(AppKit, "NSWindowCollectionBehaviorCanJoinAllSpaces"):
            w.setCollectionBehavior_(AppKit.NSWindowCollectionBehaviorCanJoinAllSpaces)
    except Exception:
        pass

    # Draw a small red circle (size according to env)
    view = AppKit.NSImageView.alloc().initWithFrame_(((0, 0), (size, size)))
    img = AppKit.NSImage.alloc().initWithSize_((size, size))
    img.lockFocus()
    try:
        AppKit.NSColor.colorWithCalibratedRed_green_blue_alpha_(1.0, 0.2, 0.2, 0.9).set()
        # Leave a 1px margin when size >= 6 for crisper edges
        inset = 1.0 if size >= 6.0 else 0.0
        path = AppKit.NSBezierPath.bezierPathWithOvalInRect_(
            ((inset, inset), (size - 2 * inset, size - 2 * inset))
        )
        path.fill()
    finally:
        img.unlockFocus()
    view.setImage_(img)
    w.setContentView_(view)
    _badge_window = w


def _screen_for_point(x: float, y: float):
    """Return the NSScreen that contains the given global point."""
    try:
        for sc in AppKit.NSScreen.screens():
            f = sc.frame()
            if (
                x >= f.origin.x
                and x <= (f.origin.x + f.size.width)
                and y >= f.origin.y
                and y <= (f.origin.y + f.size.height)
            ):
                return sc
    except Exception:
        pass
    return AppKit.NSScreen.mainScreen() if AppKit is not None else None


def _clamp_to_screen(x: float, y: float, w: float, h: float) -> tuple[float, float]:
    sc = _screen_for_point(x, y)
    if not sc:
        return x, y
    f = sc.frame()
    # Clamp so the window stays fully within the screen bounds
    x = max(f.origin.x, min(x, f.origin.x + f.size.width - w))
    y = max(f.origin.y, min(y, f.origin.y + f.size.height - h))
    return x, y


def _move_badge_to_cursor():
    if AppKit is None or _badge_window is None:
        return
    size = float(_badge_size_px())
    ox, oy = _badge_offsets()
    loc = AppKit.NSEvent.mouseLocation()
    x = float(loc.x) + ox
    y = float(loc.y) + oy
    x, y = _clamp_to_screen(x, y, size, size)
    # Use origin (bottom-left) to avoid top-left conversion shenanigans
    _badge_window.setFrameOrigin_((x, y))


def _try_move_badge_to_caret() -> bool:
    """Best-effort caret anchoring via macOS Accessibility.

    Returns True if moved using caret bounds; otherwise False (caller should fallback).
    """
    if AppKit is None or Quartz is None or _badge_window is None:
        return False
    try:
        AX = Quartz
        sys = AX.AXUIElementCreateSystemWide()
        # Focused UI element
        focused = AX.AXUIElementCopyAttributeValue(sys, AX.kAXFocusedUIElementAttribute, None)
        # PyObjC may return (err, value) or just value depending on version
        if isinstance(focused, tuple):
            err, focused = focused
            if err != 0:
                focused = None
        if not focused:
            return False

        # Selected text range (location/length)
        sel_range = AX.AXUIElementCopyAttributeValue(
            focused, AX.kAXSelectedTextRangeAttribute, None
        )
        if isinstance(sel_range, tuple):
            err, sel_range = sel_range
            if err != 0:
                sel_range = None

        # Prefer parameterized attribute: bounds for range start (length 0)
        if sel_range is not None:
            try:
                # Some apps accept an AXValue range dict; others accept NSValue-like tuple
                try_range = sel_range
                # Coerce to zero-length at start
                try:
                    start = int(try_range.location)  # NSRange-like
                except Exception:
                    start = (
                        int(getattr(try_range, "location", 0))
                        if hasattr(try_range, "location")
                        else int(
                            try_range[0]
                            if isinstance(try_range, (list, tuple)) and try_range
                            else 0
                        )
                    )
                zero_range = (start, 0)
                bounds = AX.AXUIElementCopyParameterizedAttributeValue(
                    focused, AX.kAXBoundsForRangeParameterizedAttribute, zero_range, None
                )
                if isinstance(bounds, tuple):
                    err, bounds = bounds
                    if err != 0:
                        bounds = None
                if bounds is not None:
                    # Expect a CGRect encoded as AXValue/NSValue; try to unpack
                    try:
                        # PyObjC usually bridges AXValue(CGRect) to NSValue-like with rectValue
                        if hasattr(bounds, "rectValue"):
                            r = bounds.rectValue()
                            bx, by = (
                                float(r.origin.x),
                                float(r.origin.y),
                            )
                        else:
                            # Fallback if we got a mapping/tuple
                            r = getattr(bounds, "__dict__", {}) or bounds
                            bx = float(r.get("x", r.get("origin", {}).get("x", 0.0)))
                            by = float(r.get("y", r.get("origin", {}).get("y", 0.0)))
                    except Exception:
                        bx = by = 0.0
                    size = float(_badge_size_px())
                    ox, oy = _badge_offsets()
                    # Place near the leading edge of the caret rect
                    x = bx + ox
                    y = by + oy
                    x, y = _clamp_to_screen(x, y, size, size)
                    _badge_window.setFrameOrigin_((x, y))
                    return True
            except Exception:
                pass

        # Fallback: use frame of focused element (rough approximation)
        frame_val = AX.AXUIElementCopyAttributeValue(focused, AX.kAXFrameAttribute, None)
        if isinstance(frame_val, tuple):
            err, frame_val = frame_val
            if err != 0:
                frame_val = None
        if frame_val is not None:
            try:
                if hasattr(frame_val, "rectValue"):
                    r = frame_val.rectValue()
                    bx, by = (
                        float(r.origin.x),
                        float(r.origin.y),
                    )
                else:
                    r = getattr(frame_val, "__dict__", {}) or frame_val
                    bx = float(r.get("x", r.get("origin", {}).get("x", 0.0)))
                    by = float(r.get("y", r.get("origin", {}).get("y", 0.0)))
            except Exception:
                bx = by = 0.0
            size = float(_badge_size_px())
            ox, oy = _badge_offsets()
            x = bx + ox
            y = by + oy
            x, y = _clamp_to_screen(x, y, size, size)
            _badge_window.setFrameOrigin_((x, y))
            return True
    except Exception:
        # Any failure: caller will fallback to cursor
        pass
    return False


def _current_anchor() -> str:
    """Decide where to anchor the badge: 'cursor' or 'caret'.

    If MODE is 'hands_off' → cursor; otherwise default to caret (override via BADGE_ANCHOR).
    """
    mode = (os.environ.get("MODE", "hold") or "hold").strip().lower()
    if mode == "hands_off":
        return "cursor"
    # Allow override
    return (os.environ.get("BADGE_ANCHOR", "caret") or "caret").strip().lower()


def _move_badge():
    if _current_anchor() == "caret":
        if _try_move_badge_to_caret():
            return
    _move_badge_to_cursor()


def show_hold_badge():
    if not hold_indicator_enabled() or AppKit is None:
        return

    def _show():
        global _badge_timer, _badge_timer_target
        _ensure_badge_window()
        if _badge_window is None:
            return
        _move_badge()
        _badge_window.orderFrontRegardless()
        # Update position periodically while visible
        if _badge_timer is None:
            try:
                # Create a tiny NSObject target with a tick_ method
                class _BadgeTarget(AppKit.NSObject):
                    def tick_(self, _timer):  # noqa: N802 (Objective-C selector style)
                        try:
                            _move_badge()
                        except Exception:
                            pass

                _badge_timer_target = _BadgeTarget.new()
                _badge_timer = (
                    AppKit.NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                        0.15, _badge_timer_target, "tick:", None, True
                    )
                )
            except Exception:
                # Fallback: at least ask the window to redraw
                _badge_timer = (
                    AppKit.NSTimer.scheduledTimerWithTimeInterval_target_selector_userInfo_repeats_(
                        0.3, _badge_window, "display:", None, True
                    )
                )

    if NSThread is not None and not NSThread.isMainThread() and NSOperationQueue is not None:
        NSOperationQueue.mainQueue().addOperationWithBlock_(_show)
    else:
        _show()


def hide_hold_badge():
    def _hide():
        global _badge_timer, _badge_timer_target
        if _badge_timer is not None and AppKit is not None:
            _badge_timer.invalidate()
            _badge_timer = None
            _badge_timer_target = None
        if _badge_window is not None:
            _badge_window.orderOut_(None)

    if NSThread is not None and not NSThread.isMainThread() and NSOperationQueue is not None:
        NSOperationQueue.mainQueue().addOperationWithBlock_(_hide)
    else:
        _hide()


def _coerce_ax_value(result):
    if isinstance(result, tuple):
        err, value = result
        if err != 0:
            return None
        return value
    return result


def _ax_string_attr(element, attr):
    try:
        val = _coerce_ax_value(Quartz.AXUIElementCopyAttributeValue(element, attr, None))
        if val is None:
            return None
        return str(val)
    except Exception:
        return None


def _ax_bool_attr(element, attr):
    try:
        val = _coerce_ax_value(Quartz.AXUIElementCopyAttributeValue(element, attr, None))
        return bool(val)
    except Exception:
        return False


def _ax_attribute_settable(element, attr) -> bool:
    try:
        return bool(Quartz.AXUIElementIsAttributeSettable(element, attr))
    except Exception:
        return False


def focused_element_accepts_text() -> bool:
    """Best-effort check whether the currently focused element is text-editable.

    Returns True if we are reasonably confident input is accepted. If unsure,
    defaults to True (fail-open) to avoid breaking workflows.
    """

    if Quartz is None:
        return True

    try:
        system_elem = Quartz.AXUIElementCreateSystemWide()
        focused = _coerce_ax_value(
            Quartz.AXUIElementCopyAttributeValue(
                system_elem, Quartz.kAXFocusedUIElementAttribute, None
            )
        )
        if not focused:
            return False

        role = _ax_string_attr(focused, Quartz.kAXRoleAttribute)
        if role in _EDITABLE_ROLES:
            return True

        subrole = _ax_string_attr(focused, Quartz.kAXSubroleAttribute)
        if subrole in _EDITABLE_SUBROLES:
            return True

        editable_ancestor = _coerce_ax_value(
            Quartz.AXUIElementCopyAttributeValue(
                focused, getattr(Quartz, "kAXEditableAncestorAttribute", "AXEditableAncestor"), None
            )
        )
        if editable_ancestor:
            return True

        if _ax_attribute_settable(focused, Quartz.kAXValueAttribute):
            return True

        supports_attr = getattr(
            Quartz, "kAXSupportsTextSelectionAttribute", "AXSupportsTextSelection"
        )
        if _ax_bool_attr(focused, supports_attr):
            return True

        return False
    except Exception as exc:  # pragma: no cover - depends on OS
        logging.debug(f"Accessibility text check skipped: {exc}")
        return True
