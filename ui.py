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
except Exception:  # Allow importing ui helpers in non-macOS test envs
    AppKit = None  # type: ignore
    Quartz = None  # type: ignore
import logging
import os
import time

# configure logging
logging.basicConfig(
    level=logging.INFO, format="%(asctime)s - %(levelname)s - %(message)s"
)

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
    def set(app, glyph: str):
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
            # logging.debug(f"Menu icon set to: {glyph}")
        else:
            logging.warning("Attempted to set menu icon, but app object was None.")

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
        MenuIcon.set(app, ICON_SUCCESS)
        logging.info(f"UI State: Success ({ICON_SUCCESS})")
        # schedule the reset_ method to be called on the app instance after 1 second
        # uses nstimer for integration with the macos run loop used by rumps
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
    lang = (getattr(cfg, 'language', None) or 'auto')
    fmt = 'ON' if getattr(cfg, 'format_enabled', False) else 'OFF'
    wurl = getattr(cfg, 'whisper_url', '') or 'local'
    lurl = getattr(cfg, 'llm_url', '') or 'local'
    return [
        f"Language: {lang}",
        f"Formatting: {fmt}",
        f"Whisper URL: {wurl}",
        f"LLM URL: {lurl}",
    ]


def toggles_help_message(lang: str = 'en') -> str:
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
    try:
        # 1. copy text to pasteboard
        pasteboard = AppKit.NSPasteboard.generalPasteboard()
        pasteboard.clearContents()  # clear existing contents
        # declare types and set string
        # nsstringpboardtype is the standard type for plain text
        pasteboard.declareTypes_owner_([AppKit.NSStringPboardType], None)
        success = pasteboard.setString_forType_(text, AppKit.NSStringPboardType)
        if not success:
            logging.error("Failed to set string on pasteboard.")
            return
        logging.info("Text successfully copied to clipboard.")

        # 2. simulate cmd+v keypress
        # create an event source
        # kcgeventsourcestatecombinedsessionstate reflects the current user session state
        source = Quartz.CGEventSourceCreate(
            Quartz.kCGEventSourceStateCombinedSessionState
        )
        if not source:
            logging.error("Failed to create CGEventSource.")
            return

        # key code for 'v' is 9
        v_keycode = 9

        # create key down event for cmd+v
        event_down = Quartz.CGEventCreateKeyboardEvent(
            source, v_keycode, True
        )  # true for key down
        if not event_down:
            logging.error("Failed to create key down event.")
            return
        # set the command flag
        Quartz.CGEventSetFlags(event_down, Quartz.kCGEventFlagMaskCommand)

        # create key up event for cmd+v
        event_up = Quartz.CGEventCreateKeyboardEvent(
            source, v_keycode, False
        )  # false for key up
        if not event_up:
            logging.error("Failed to create key up event.")
            return
        # set the command flag
        Quartz.CGEventSetFlags(event_up, Quartz.kCGEventFlagMaskCommand)

        # post events to the system event stream
        # kcghideventtap is the location for hardware input events
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, event_down)
        # add a small delay between down and up, mimicking human typing
        time.sleep(0.01)
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, event_up)

        logging.info("Command+V keypress simulated successfully.")

    except Exception as e:
        logging.error(f"Error during paste operation: {e}", exc_info=True)
        logging.error(
            "Ensure Accessibility permissions are granted for the application."
        )


def start_sound():
    """Play a soft, non-error start sound.

    Tries macOS system sounds (NSSound) with low volume. The sound and volume
    can be customized via env vars: SOUND_NAME (e.g., 'Tink', 'Pop') and
    SOUND_VOLUME (0.0–1.0). Falls back to a quiet ASCII bell if AppKit is not
    available.
    """
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
