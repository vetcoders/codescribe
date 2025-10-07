# main.py
#
# purpose: entry point for the VistaScribe application. initializes and runs the
#          macos menu bar app, manages the application's state machine, and
#          coordinates interactions between hotkey detection, audio recording,
#          transcription, formatting, and ui updates.
#
# dependencies: asyncio (core async operations)
#               rumps (macos menu bar app framework)
#               hotkeys.py (provides hotkey events)
#               audio.py (provides recorder class)
#               stt.py (provides transcription function)
#               llm.py (provides text formatting function)
#               ui.py (provides menu icon updates and paste function)
#               logging (for application-level logging)
#
# key components: VistaScribe class (subclass of rumps.app)
#                 state machine logic (idle, rec_hold, rec_toggle, busy)
#                 async worker task to process hotkey events
#
# design rationale: uses rumps for simple menu bar integration on macos.
#                   employs an async event loop to handle concurrent tasks like
#                   listening for hotkeys and processing audio/api calls without
#                   blocking the main thread. a simple state machine ensures
#                   predictable behavior based on user input and processing status.
#
import asyncio
import fcntl
import importlib
import logging
import os
import plistlib
import queue  # for checking standard queue
import sys
import threading  # for asyncio thread

import objc  # for selector
import requests
import rumps

import diag  # developer diagnostics
import first_run
import llm  # runtime toggle for formatting
from audio import Recorder
from config import Config, load_config, save_config, update_env_vars

# import our modules
from hotkeys import (
    events as hk_events,
    hold_mods_label as hotkeys_hold_mods_label,
    is_hold_exclusive as hotkeys_is_hold_exclusive,
    set_hold_exclusive as hotkeys_set_hold_exclusive,
    set_hold_mods as hotkeys_set_hold_mods,
    start as hotkeys_start,
    stop as hotkeys_stop,
)
from llm import format_text
from stt import get_language, set_language, transcribe
from ui import (
    MenuIcon,
    backend_status_labels,
    config_labels,
    hide_hold_badge,
    paste_text,
    show_hold_badge,
    start_sound,
    toggles_help_message,
)

# --- global state ---

# application state machine
# possible states: idle, rec_hold, rec_toggle, busy
STATE = "IDLE"
# Delay for Ctrl-hold start (ms). Prevents accidental triggers when using Control.
HOLD_START_DELAY_MS = int(
    os.environ.get(
        "CTRL_HOLD_DELAY_MS",
        os.environ.get("HOLD_START_DELAY_MS", "500"),
    )
)
# Sound confirmation toggle
BEEP_ON_START = os.environ.get("BEEP_ON_START", "1").lower() not in (
    "0",
    "false",
    "no",
    "off",
)
# Task handle for delayed hold start
_hold_start_task = None
# global recorder instance
recorder = Recorder()

# configure logging (set level for the entire application)
# consider moving this to a dedicated config area if app grows
log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
logging.basicConfig(level=log_level, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s")
logger = logging.getLogger(__name__)


# --- singleton lock ---
def acquire_lock():
    """Acquire a lock to ensure only one instance of the application runs."""
    lock_file_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".vista_scribe.lock")

    try:
        lock_file = open(lock_file_path, "w")
        fcntl.lockf(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # Write the process ID to the lock file
        lock_file.write(str(os.getpid()))
        lock_file.flush()
        return lock_file
    except OSError:
        # Another instance is already running
        logger.error("Another instance of Vista Scribe is already running.")
        return None


# --- core logic functions ---


def _set_status(app: rumps.App, text: str) -> None:
    """Update the top status item in the tray menu."""
    try:
        for key in list(app.menu.keys()):
            try:
                item = app.menu[key]
                if isinstance(item, rumps.MenuItem) and str(key).startswith("Status:"):
                    item.title = f"Status: {text}"
                    return
            except Exception:
                pass
        # Fallback: prepend a new status item
        app.menu.insert(0, rumps.MenuItem(f"Status: {text}"))
    except Exception:
        logger.debug("Failed to update status label")


async def finish_recording(app: rumps.App):
    """handles the process after recording stops.

    stops recorder, transcribes, formats, pastes, updates ui, and resets state.
    includes error handling for transcription and formatting steps.

    args:
        app (rumps.app): the main application instance.
    """
    global STATE
    if STATE == "IDLE" or STATE == "BUSY":  # shouldn't happen, but safeguard
        logger.warning(f"finish_recording called unexpectedly in state: {STATE}")
        return

    logger.info(f"Finishing recording (current state: {STATE})")
    previous_state = STATE
    STATE = "BUSY"
    MenuIcon.think(app)
    _set_status(app, "Processing…")

    try:
        # 1. stop recording and get audio path
        path = await recorder.stop()
        if not path:
            logger.error("Audio recording failed or produced no file.")
            # reset state without success indication
            MenuIcon.set(app, MenuIcon.IDLE)
            _set_status(app, "Ready")
            STATE = "IDLE"
            return

        # 2. transcribe audio
        logger.info(f"Transcribing audio file: {path}")
        raw_text = await transcribe(path)
        if raw_text is None:
            logger.error("Transcription failed.")
            # optionally notify user via menu?
            MenuIcon.set(app, MenuIcon.IDLE)  # reset state
            _set_status(app, "Ready")
            STATE = "IDLE"
            # clean up the temp file if transcription fails
            if os.path.exists(path):
                try:
                    os.remove(path)  # <-- re-enable deletion
                    logger.info(f"Cleaned up temp file (finally block): {path}")
                except OSError as e:
                    logger.error(f"Failed to remove temp file {path} in finally block: {e}")
            return
        logger.info(f"Raw transcript: '{raw_text[:50]}...'")

        # 3. format text (optional, proceed even if formatting fails?)
        logger.info("Formatting transcript...")
        formatted_text = await format_text(raw_text)
        if formatted_text is None:
            logger.warning("Text formatting failed. Pasting raw transcript instead.")
            text_to_paste = raw_text  # fallback to raw text
        else:
            text_to_paste = formatted_text
            logger.info(f"Formatted text: '{text_to_paste[:50]}...'")

        # 4. paste text
        if text_to_paste:
            paste_text(text_to_paste)
        else:
            logger.warning("No text available to paste after processing.")

        # 5. indicate success and reset state
        MenuIcon.success(app)  # success() handles timer to reset icon later
        STATE = "IDLE"
        _set_status(app, "Ready")
        logger.info("Processing finished successfully.")

    except Exception as e:
        logger.error(f"An unexpected error occurred during finish_recording: {e}", exc_info=True)
        MenuIcon.set(app, MenuIcon.IDLE)  # reset state on error
        _set_status(app, "Ready")
        STATE = "IDLE"
    finally:
        # ensure temp file is cleaned up if it still exists (e.g., if formatting failed)
        if "path" in locals() and path and os.path.exists(path):
            try:
                os.remove(path)  # <-- re-enable deletion
                logger.info(f"Cleaned up temp file (finally block): {path}")
            except OSError as e:
                logger.error(f"Failed to remove temp file {path} in finally block: {e}")
        # Hide the hold badge in any case
        try:
            hide_hold_badge()
        except Exception:
            pass


async def _start_recording_after_delay(app: rumps.App):
    """Start recording after the configured hold delay, unless cancelled.

    This function is scheduled when Ctrl is pressed. If Ctrl is released
    before the delay elapses, the task is cancelled and no recording starts.
    """
    global STATE
    try:
        await asyncio.sleep(HOLD_START_DELAY_MS / 1000.0)
        # Start only if we are still idle (wasn't cancelled and nothing else started)
        if STATE != "IDLE":
            return
        logger.info(">>> Attempting to start recording (Hold Delay elapsed)...")
        await recorder.start()
        MenuIcon.listen(app)
        _set_status(app, "Listening…")
        if BEEP_ON_START:
            start_sound()
        try:
            show_hold_badge()
        except Exception:
            pass
        STATE = "REC_HOLD"
        logger.info("State transition: IDLE -> REC_HOLD")
    except asyncio.CancelledError:
        logger.info("Hold-start delay cancelled before firing")
        raise
    except Exception as e:
        logger.error(f"Failed to start recording after hold delay: {e}", exc_info=True)
        MenuIcon.set(app, MenuIcon.IDLE)
        STATE = "IDLE"


async def handle_hotkey_event(app: rumps.App, key_type: str, action: str):
    """handles incoming hotkey events based on the current application state.

    manages state transitions (idle -> rec_hold/rec_toggle -> busy -> idle).

    args:
        app (rumps.app): the main application instance.
        key_type (str): 'hold' or 'toggle'.
        action (str): 'down', 'up', or 'press'.
    """
    global STATE
    logger.info(f"--- Handling event: type={key_type}, action={action}, current_state={STATE} ---")
    logger.debug(f"Hotkey event received: type={key_type}, action={action}, current_state={STATE}")

    if STATE == "BUSY":
        logger.warning("Hotkey event ignored: application is busy.")
        return

    if key_type == "hold":
        global _hold_start_task
        if action == "down" and STATE == "IDLE":
            # Cancel any previous pending task (safety)
            if _hold_start_task and not _hold_start_task.done():
                _hold_start_task.cancel()
                try:
                    await _hold_start_task
                except Exception:
                    pass
            logger.info(f"Scheduling hold-start after {HOLD_START_DELAY_MS}ms…")
            _hold_start_task = asyncio.create_task(_start_recording_after_delay(app))
        elif action == "up":
            # If recording already started via hold, finish it
            if STATE == "REC_HOLD":
                logger.info(">>> Attempting to finish recording (Hold Up)...")
                logger.info("Hold key released, initiating finish sequence.")
                await finish_recording(app)
            else:
                # Cancel pending delayed start if not yet started
                if _hold_start_task and not _hold_start_task.done():
                    _hold_start_task.cancel()
                    try:
                        await _hold_start_task
                    except Exception:
                        pass
                    _hold_start_task = None
                # Remain IDLE
                logger.info("Hold released before delay; no recording started.")

    elif key_type == "toggle" and action == "press":
        if STATE == "IDLE":
            try:
                logger.info(">>> Attempting to start recording (Toggle Press IDLE)...")
                await recorder.start()
                # recorder started successfully, now update ui and state
                logger.info(">>> Recorder started, attempting to set icon to listen...")
                MenuIcon.listen(app)
                _set_status(app, "Listening…")
                if BEEP_ON_START:
                    start_sound()
                try:
                    # Reuse the small badge indicator during Hands-Off as well
                    show_hold_badge()
                except Exception:
                    pass
                STATE = "REC_TOGGLE"
                logger.info("State transition: IDLE -> REC_TOGGLE")
            except Exception as e:
                # This catches errors from recorder.start() only
                logger.error(f"Failed to start recording on toggle-press: {e}", exc_info=True)
                # Reset state and attempt cleanup if start failed
                MenuIcon.set(app, MenuIcon.IDLE)
                STATE = "IDLE"
                # Attempt graceful stop to clean up recorder state if needed
                if recorder._stream:  # Check if stream was partially created
                    logger.warning("Attempting recorder cleanup after start failure...")
                    try:
                        await recorder.stop()
                    except Exception as stop_e:
                        logger.error(f"Error during cleanup recorder.stop: {stop_e}")
        elif STATE == "REC_TOGGLE":
            logger.info(">>> Attempting to finish recording (Toggle Press REC_TOGGLE)...")
            logger.info("Toggle key pressed again, initiating finish sequence.")
            await finish_recording(app)
            # state becomes busy then idle within finish_recording


# --- rumps application class ---


class VistaScribe(rumps.App):
    """macOS menu bar application class using rumps.

    Integrates hotkey listening, state management, and UI updates.
    Runs asyncio operations in a separate thread.
    """

    def __init__(self):
        """Initializes the rumps app, queue timer, and asyncio thread setup."""
        super().__init__(MenuIcon.IDLE, quit_button=None)

        # Try to set a tray icon image (keeps state glyphs out of the title area)
        try:
            from path_utils import normalize_model_path  # lazy import to avoid cycles

            icon_env = os.environ.get("TRAY_ICON")
            repo_root = os.path.dirname(os.path.abspath(__file__))
            default_icon = os.path.join(repo_root, "assets", "icon.png")
            candidate = icon_env or default_icon
            if candidate:
                # Normalize '/Users' → '/users' on macOS when that path exists
                norm = normalize_model_path(candidate) or candidate
                if os.path.isfile(norm):
                    self.icon = norm
                    # Hide title text when an icon is present
                    self.title = ""
                    logger.info(f"Tray icon set: {norm}")
                    # In DEV_MODE force a visible glyph right away so we see the status item
                    if os.environ.get("DEV_MODE", "0").lower() in ("1", "true", "yes", "on"):
                        MenuIcon.set(self, MenuIcon.IDLE)
        except Exception as e:
            logger.warning(f"Tray icon setup skipped: {e}")

        # Initialize menu with app status and controls
        self.hotkeys_enabled = True  # Default state, will be updated in run_loop

        self.menu = [
            "Status: Initializing...",
            None,  # Separator
            "Enable Hotkeys",
            "Enable Formatting",
            None,  # Separator
            "Language: Auto",
            "Language: Polish (PL)",
            "Language: English (EN)",
            "What do these toggles do?",
            None,  # Separator
            "Mode",
            None,  # Separator
            "Hotkey Settings",
            "Start at Login",
            "Feedback",
            None,  # Separator
            "Models",
            None,  # Separator
            "Backends",  # placeholder; populated below
            None,  # Separator
            "Open System Accessibility Settings...",
            None,  # Separator
            "Quit",
        ]

        # Set callbacks
        self.menu["Enable Hotkeys"].set_callback(self._toggle_hotkeys)
        self.menu["Enable Formatting"].set_callback(self._toggle_formatting)
        self.menu["Language: Auto"].set_callback(self._set_language_auto)
        self.menu["Language: Polish (PL)"].set_callback(self._set_language_pl)
        self.menu["Language: English (EN)"].set_callback(self._set_language_en)
        self.menu["What do these toggles do?"].set_callback(self._show_toggles_help)
        self.menu["Open System Accessibility Settings..."].set_callback(self._open_accessibility)
        self.menu["Quit"].set_callback(self._quit_app)
        self.menu["Start at Login"].set_callback(self._toggle_login_item)

        # Mode submenu (Hold / Hands-Off / Advanced)
        cur_mode = (os.environ.get("MODE", "hold") or "hold").strip().lower()
        self.item_mode_hold = rumps.MenuItem("Hold", callback=lambda _s: self._set_mode("hold"))
        self.item_mode_handoff = rumps.MenuItem(
            "Hands-Off", callback=lambda _s: self._set_mode("hands_off")
        )
        self.item_mode_adv = rumps.MenuItem(
            "Advanced (stub)", callback=lambda _s: self._set_mode("advanced")
        )
        self.item_mode_save = rumps.MenuItem("Save Mode to .env", callback=self._save_mode_env)
        self.menu["Mode"] = [
            rumps.MenuItem("Current: " + cur_mode.replace("_", " ")),  # read-only label
            None,
            self.item_mode_hold,
            self.item_mode_handoff,
            self.item_mode_adv,
            None,
            self.item_mode_save,
        ]
        # Ensure submenu is enabled (avoid greyed-out appearance)
        try:
            self.menu["Mode"].set_callback(lambda _s: None)
        except Exception:
            pass
        self._refresh_mode_menu()

        # Hotkey settings submenu (predefined hold combos)
        self.item_hold_ctrl = rumps.MenuItem("Hold: Ctrl only", callback=self._set_hold_ctrl)
        self.item_hold_ctrl_opt = rumps.MenuItem(
            "Hold: Ctrl+Option (recommended)", callback=self._set_hold_ctrl_opt
        )
        self.item_hold_ctrl_shift = rumps.MenuItem(
            "Hold: Ctrl+Shift", callback=self._set_hold_ctrl_shift
        )
        self.item_hold_ctrl_cmd = rumps.MenuItem(
            "Hold: Ctrl+Command", callback=self._set_hold_ctrl_cmd
        )
        self.item_hold_excl = rumps.MenuItem(
            "Exclusive (ignore extra modifiers)", callback=self._toggle_hold_exclusive
        )
        self.item_hold_current = rumps.MenuItem(
            "Hold: " + hotkeys_hold_mods_label(), callback=lambda _s: None
        )
        try:
            import hotkeys as hk_mod

            cur_trig = hk_mod.get_toggle_trigger()
        except Exception:
            cur_trig = "double_option"
        self.item_toggle_current = rumps.MenuItem(
            "Toggle: " + cur_trig.replace("_", " "), callback=lambda _s: None
        )
        self.item_customize_hotkeys = rumps.MenuItem(
            "Configure Hotkeys…", callback=self._customize_hotkeys
        )
        self.menu["Hotkey Settings"] = [
            self.item_hold_current,
            self.item_toggle_current,
            None,
            self.item_hold_ctrl,
            self.item_hold_ctrl_opt,
            self.item_hold_ctrl_shift,
            self.item_hold_ctrl_cmd,
            rumps.MenuItem("Save Hotkeys to .env", callback=self._save_hotkeys_env),
            None,
            self.item_hold_excl,
            None,
            self.item_customize_hotkeys,
        ]
        try:
            self.menu["Hotkey Settings"].set_callback(lambda _s: None)
        except Exception:
            pass
        self._refresh_hold_menu()

        # Feedback (start sound) submenu
        self.item_beep = rumps.MenuItem("Enable Start Sound", callback=self._toggle_beep)
        self.item_sound_tink = rumps.MenuItem(
            "Sound: Tink", callback=lambda _s: self._set_sound_name("Tink")
        )
        self.item_sound_pop = rumps.MenuItem(
            "Sound: Pop", callback=lambda _s: self._set_sound_name("Pop")
        )
        self.item_volume = rumps.MenuItem("Set Volume…", callback=self._set_sound_volume)
        self.item_sound_save = rumps.MenuItem(
            "Save Feedback to .env", callback=self._save_feedback_env
        )
        self.menu["Feedback"] = [
            self.item_beep,
            None,
            self.item_sound_tink,
            self.item_sound_pop,
            self.item_volume,
            None,
            self.item_sound_save,
        ]
        try:
            self.menu["Feedback"].set_callback(lambda _s: None)
        except Exception:
            pass
        # Reflect current env
        self._refresh_feedback_menu()

        # Models submenu: download & select
        self.item_model_current = rumps.MenuItem("Current: —")
        self.item_use_small = rumps.MenuItem(
            "Use Whisper: Small (Bundled)", callback=lambda _s: self._set_model_variant("small")
        )
        self.item_use_medium = rumps.MenuItem(
            "Use Whisper: Medium", callback=lambda _s: self._set_model_variant("medium")
        )
        self.item_use_lv3 = rumps.MenuItem(
            "Use Whisper: Large v3", callback=lambda _s: self._set_model_variant("large-v3")
        )
        self.item_use_lvt = rumps.MenuItem(
            "Use Whisper: Large v3 Turbo",
            callback=lambda _s: self._set_model_variant("large-v3-turbo"),
        )
        self.item_dl_small = rumps.MenuItem(
            "Download Whisper: Small", callback=lambda _s: self._download_models("small-mlx")
        )
        self.item_dl_medium = rumps.MenuItem(
            "Download Whisper: Medium", callback=lambda _s: self._download_models("medium")
        )
        self.item_dl_lv3 = rumps.MenuItem(
            "Download Whisper: Large v3", callback=lambda _s: self._download_models("large-v3")
        )
        self.item_dl_lvt = rumps.MenuItem(
            "Download Whisper: Large v3 Turbo",
            callback=lambda _s: self._download_models("large-v3-turbo"),
        )
        self.item_open_models = rumps.MenuItem(
            "Open Models Folder", callback=self._open_models_folder
        )
        self.menu["Models"] = [
            self.item_model_current,
            None,
            self.item_use_small,
            self.item_use_medium,
            self.item_use_lv3,
            self.item_use_lvt,
            None,
            self.item_dl_small,
            self.item_dl_medium,
            self.item_dl_lv3,
            self.item_dl_lvt,
            None,
            self.item_open_models,
        ]
        try:
            self.menu["Models"].set_callback(lambda _s: None)
        except Exception:
            pass
        self._refresh_models_menu()

        # --- Mini config tool (Backends) ---
        self.cfg: Config = load_config()
        # Create items to be updated dynamically
        self.item_stt_status = rumps.MenuItem("STT: OFF")
        self.item_llm_status = rumps.MenuItem("LLM: OFF")
        self.item_w_url = rumps.MenuItem("Whisper URL: local", callback=self._edit_whisper_url)
        self.item_l_url = rumps.MenuItem("LLM URL: local", callback=self._edit_llm_url)
        self.item_check = rumps.MenuItem("Check Backends", callback=self._check_backends)
        self.menu["Backends"] = [
            self.item_stt_status,
            self.item_llm_status,
            None,
            self.item_w_url,
            self.item_l_url,
            self.item_check,
        ]
        try:
            self.menu["Backends"].set_callback(lambda _s: None)
        except Exception:
            pass
        # Apply env and update labels
        self._apply_cfg_env()
        self._update_backend_menu_labels()

        # Disable menu items initially until we know hotkeys status
        self.menu["Enable Hotkeys"].state = False
        # Reflect current formatting toggle
        self.menu["Enable Formatting"].state = llm.FORMAT_ENABLED
        # Initialize language menu state
        current_lang = get_language()
        self.menu["Language: Auto"].state = current_lang is None
        self.menu["Language: Polish (PL)"].state = current_lang == "pl"
        self.menu["Language: English (EN)"].state = current_lang == "en"

        self.event_queue = hk_events()  # get the standard queue
        self.async_loop = None
        self.async_thread = None
        self.queue_timer = rumps.Timer(self.poll_queue, 0.05)  # poll queue every 50ms
        logger.info("Vista Scribe App initialized.")
        # Developer diagnostics: preflight snapshot if DEV_MODE enabled
        try:
            if os.environ.get("DEV_MODE", "0").lower() in ("1", "true", "yes", "on"):
                info = diag.run_preflight(logger)
                diag.write_snapshot(info, os.path.dirname(os.path.abspath(__file__)))
        except Exception as e:
            logger.debug(f"Diagnostics failed: {e}")
        # Minimal first-run setup: config + sensible defaults (non-blocking)
        try:
            first_run.ensure_config_and_permissions()
        except Exception as e:
            logger.warning(f"First-run setup skipped: {e}")

        # Reflect Start at Login state
        try:
            self.menu["Start at Login"].state = self._is_login_installed()
        except Exception:
            pass

    # --- Start at Login via LaunchAgent ---
    def _login_plist_path(self) -> str:
        home = os.path.expanduser("~")
        return os.path.join(home, "Library", "LaunchAgents", "com.vistascribe.tray.plist")

    def _is_login_installed(self) -> bool:
        return os.path.exists(self._login_plist_path())

    def _toggle_login_item(self, _sender):
        try:
            if self._is_login_installed():
                self._remove_login_agent()
            else:
                self._install_login_agent()
        except Exception as e:
            logger.error(f"Login item toggle failed: {e}")
        try:
            self.menu["Start at Login"].state = self._is_login_installed()
        except Exception:
            pass

    def _install_login_agent(self):
        path = self._login_plist_path()
        os.makedirs(os.path.dirname(path), exist_ok=True)
        app_repo = "/Applications/VistaScribe.app/Contents/Resources/Repo"
        cmd = (
            f"cd '{app_repo}' && ./scripts/quickstart_mac.sh --mode both --daemon --log "
            f"'$HOME/Library/Logs/VistaScribe.app.log'"
        )
        data = {
            "Label": "com.vistascribe.tray",
            "ProgramArguments": ["/bin/zsh", "-lc", cmd],
            "RunAtLoad": True,
            "KeepAlive": False,
            "StandardOutPath": os.path.expanduser("~/Library/Logs/VistaScribe.launchd.out.log"),
            "StandardErrorPath": os.path.expanduser("~/Library/Logs/VistaScribe.launchd.err.log"),
        }
        with open(path, "wb") as f:
            plistlib.dump(data, f)
        import subprocess

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        subprocess.run(["launchctl", "load", "-w", path], check=False)
        logger.info("Installed Start at Login LaunchAgent")

    def _remove_login_agent(self):
        path = self._login_plist_path()
        import subprocess

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        try:
            os.remove(path)
        except FileNotFoundError:
            pass
        logger.info("Removed Start at Login LaunchAgent")

    # --- Hotkey Customizer (MVP) ---
    def _customize_hotkeys(self, _sender):
        try:
            import AppKit

            # Step 1: Hold selection
            alert1 = AppKit.NSAlert.new()
            alert1.setMessageText_("Hold Hotkey")
            alert1.setInformativeText_("Choose the hold combination.")
            hold_choices = [
                ("Ctrl", "ctrl"),
                ("Ctrl+Option", "ctrl+alt"),
                ("Ctrl+Shift", "ctrl+shift"),
                ("Ctrl+Command", "ctrl+cmd"),
                ("Cancel", None),
            ]
            for title, _ in hold_choices:
                alert1.addButtonWithTitle_(title)
            r1 = alert1.runModal() - 1000
            if not (0 <= r1 < len(hold_choices)):
                return
            hold_spec = hold_choices[r1][1]
            if hold_spec:
                os.environ["HOLD_MODS"] = hold_spec
                hotkeys_set_hold_mods(hold_spec)
                try:
                    update_env_vars({"HOLD_MODS": hold_spec})
                except Exception:
                    pass
                self._refresh_hold_menu()

            # Step 2: Toggle trigger
            alert2 = AppKit.NSAlert.new()
            alert2.setMessageText_("Toggle Trigger")
            alert2.setInformativeText_("Choose the hands‑off trigger.")
            trig_map = [
                ("Double Option", "double_option"),
                ("Double Right‑Option", "double_ralt"),
                ("Double Space (global)", "double_space"),
                ("Disable", "none"),
            ]
            for title, _ in trig_map:
                alert2.addButtonWithTitle_(title)
            r2 = alert2.runModal() - 1000
            if 0 <= r2 < len(trig_map):
                trig = trig_map[r2][1]
                try:
                    import hotkeys as hk_mod

                    hk_mod.set_toggle_trigger(trig)
                except Exception:
                    pass
                os.environ["TOGGLE_TRIGGER"] = trig
                try:
                    update_env_vars({"TOGGLE_TRIGGER": trig})
                except Exception:
                    pass
                self.item_toggle_current.title = "Toggle: " + trig.replace("_", " ")
        except Exception as e:
            logger.error(f"Configure hotkeys failed: {e}")

    def _refresh_feedback_menu(self):
        self.item_beep.state = BEEP_ON_START
        current = os.environ.get("SOUND_NAME", "Tink")
        self.item_sound_tink.state = current == "Tink"
        self.item_sound_pop.state = current == "Pop"

    def _refresh_hold_menu(self):
        label = hotkeys_hold_mods_label()
        try:
            self.item_hold_current.title = f"Current: {label}"
        except Exception:
            pass
        self.item_hold_ctrl.state = label == "Ctrl"
        self.item_hold_ctrl_opt.state = label == "Ctrl+Option"
        self.item_hold_ctrl_shift.state = label == "Ctrl+Shift"
        self.item_hold_ctrl_cmd.state = label == "Ctrl+Command"
        self.item_hold_excl.state = hotkeys_is_hold_exclusive()

    def _set_hold_ctrl(self, _sender):
        os.environ["HOLD_MODS"] = "ctrl"
        hotkeys_set_hold_mods("ctrl")
        try:
            update_env_vars({"HOLD_MODS": "ctrl"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _set_hold_ctrl_opt(self, _sender):
        os.environ["HOLD_MODS"] = "ctrl+alt"
        hotkeys_set_hold_mods("ctrl+alt")
        try:
            update_env_vars({"HOLD_MODS": "ctrl+alt"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _set_hold_ctrl_shift(self, _sender):
        os.environ["HOLD_MODS"] = "ctrl+shift"
        hotkeys_set_hold_mods("ctrl+shift")
        try:
            update_env_vars({"HOLD_MODS": "ctrl+shift"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _set_hold_ctrl_cmd(self, _sender):
        os.environ["HOLD_MODS"] = "ctrl+cmd"
        hotkeys_set_hold_mods("ctrl+cmd")
        try:
            update_env_vars({"HOLD_MODS": "ctrl+cmd"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _toggle_hold_exclusive(self, _sender):
        new_flag = not hotkeys_is_hold_exclusive()
        os.environ["HOLD_EXCLUSIVE"] = "1" if new_flag else "0"
        hotkeys_set_hold_exclusive(new_flag)
        try:
            update_env_vars({"HOLD_EXCLUSIVE": "1" if new_flag else "0"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _save_hotkeys_env(self, _sender):
        mods = os.environ.get("HOLD_MODS", "ctrl")
        excl = os.environ.get("HOLD_EXCLUSIVE", "1" if mods in ("ctrl", "control") else "0")
        try:
            update_env_vars({"HOLD_MODS": mods, "HOLD_EXCLUSIVE": excl})
            rumps.notification(
                title="VistaScribe",
                subtitle="Hotkeys saved",
                message=f"HOLD_MODS={mods}, EXCLUSIVE={excl}",
            )
        except Exception as e:
            logger.error(f"Failed to save hotkeys to .env: {e}")

    # --- Feedback (start sound) ---
    def _toggle_beep(self, _sender):
        global BEEP_ON_START
        BEEP_ON_START = not BEEP_ON_START
        try:
            update_env_vars({"BEEP_ON_START": "1" if BEEP_ON_START else "0"})
        except Exception:
            pass
        self._refresh_feedback_menu()

    def _set_sound_name(self, name: str):
        os.environ["SOUND_NAME"] = name
        try:
            update_env_vars({"SOUND_NAME": name})
        except Exception:
            pass
        self._refresh_feedback_menu()

    def _set_sound_volume(self, _sender):
        vol_str = os.environ.get("SOUND_VOLUME", "0.2")
        w = rumps.Window(
            message="Enter start sound volume (0.0 – 1.0)",
            default_text=vol_str,
            title="Start Sound Volume",
            ok="Save",
            cancel="Cancel",
        )
        resp = w.run()
        if resp.clicked:
            try:
                v = max(0.0, min(1.0, float(resp.text.strip())))
                os.environ["SOUND_VOLUME"] = str(v)
                try:
                    update_env_vars({"SOUND_VOLUME": str(v)})
                except Exception:
                    pass
            except Exception:
                rumps.alert(
                    title="Invalid value",
                    message="Please enter a number between 0.0 and 1.0",
                )

    def _save_feedback_env(self, _sender):
        try:
            update_env_vars(
                {
                    "BEEP_ON_START": "1" if BEEP_ON_START else "0",
                    "SOUND_NAME": os.environ.get("SOUND_NAME", "Tink"),
                    "SOUND_VOLUME": os.environ.get("SOUND_VOLUME", "0.2"),
                }
            )
            rumps.notification(
                title="VistaScribe",
                subtitle="Feedback saved",
                message="Sound settings persisted to .env",
            )
        except Exception as e:
            logger.error(f"Failed to save feedback to .env: {e}")

    # --- Models management ---
    def _download_models(self, variant: str):
        import subprocess

        repo_root = os.path.dirname(os.path.abspath(__file__))
        script = os.path.join(repo_root, "scripts", "get_models.py")
        # Extend script to support 'large-v3' gracefully
        if variant == "large-v3":
            variant_arg = "large-v3"
        else:
            variant_arg = variant

        if not os.path.isfile(script):
            rumps.alert(
                title="Download Error",
                message="Model downloader script not found in app bundle.",
            )
            return

        _set_status(self, f"Downloading {variant_arg}…")

        def _run():
            try:
                subprocess.run(
                    [sys.executable, script, "--whisper", variant_arg],
                    cwd=repo_root,
                    check=False,
                )
            finally:
                _set_status(self, "Ready")

        threading.Thread(target=_run, daemon=True).start()

    def _open_models_folder(self, _sender):
        import subprocess

        repo_root = os.path.dirname(os.path.abspath(__file__))
        models_dir = os.path.join(repo_root, "models")
        os.makedirs(models_dir, exist_ok=True)
        try:
            subprocess.run(["open", models_dir])
        except Exception:
            rumps.alert(title="Open Folder", message=models_dir)

    def _refresh_models_menu(self):
        try:
            import stt as stt_mod

            cur = stt_mod.get_current_variant()
        except Exception:
            cur = "unknown"
        label = {
            "medium": "Medium",
            "large-v3": "Large v3",
            "large-v3-turbo": "Large v3 Turbo",
            "remote": "Remote",
        }.get(cur, cur)
        self.item_model_current.title = f"Current: {label}"
        self.item_use_small.state = cur == "small"
        self.item_use_medium.state = cur == "medium"
        self.item_use_lv3.state = cur == "large-v3"
        self.item_use_lvt.state = cur == "large-v3-turbo"

    def _set_model_variant(self, variant: str):
        try:
            import stt as stt_mod

            ok = stt_mod.set_variant(variant)
            if ok:
                # Persist env for next runs
                try:
                    update_env_vars(
                        {
                            "WHISPER_VARIANT": variant,
                            "WHISPER_DIR": os.environ.get("WHISPER_DIR", ""),
                        }
                    )
                except Exception:
                    pass
                rumps.notification(
                    title="VistaScribe",
                    subtitle="Model switched",
                    message=variant,
                )
                self._refresh_models_menu()
            else:
                rumps.alert(
                    title="Switch failed",
                    message=(
                        "Could not switch model. Make sure the variant is "
                        "downloaded (Models → Download)."
                    ),
                )
        except Exception as e:
            logger.error(f"Failed to switch model: {e}")

    def poll_queue(self, _timer):
        """periodically called by rumps.timer to check the event queue.

        pulls events from the standard queue (populated by hotkeys.py)
        and schedules the async handler on the dedicated asyncio loop thread.
        """
        if not self.async_loop or not self.async_loop.is_running():
            # logger.debug("Async loop not ready, skipping queue poll.")
            return

        try:
            while not self.event_queue.empty():
                key_type, action = self.event_queue.get_nowait()
                logger.info(f"--- Polled event: type={key_type}, action={action} ---")
                # schedule handle_hotkey_event to run in the asyncio thread
                asyncio.run_coroutine_threadsafe(
                    handle_hotkey_event(self, key_type, action), self.async_loop
                )
                self.event_queue.task_done()
        except queue.Empty:
            pass  # no events in queue, normal
        except Exception as e:
            logger.error(f"Error polling queue or scheduling handler: {e}", exc_info=True)

    def _run_async_loop(self):
        """target function for the asyncio thread.
        sets up the event loop for the thread and runs it forever.
        """
        self.async_loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.async_loop)
        logger.info("Asyncio event loop starting in background thread.")
        self.async_loop.run_forever()
        # loop has stopped
        self.async_loop.close()
        logger.info("Asyncio event loop closed.")

    def _quit_app(self, _sender):
        """Cleanly quits the application.

        Stops hotkeys, cleans up event taps, stops the timer,
        signals the asyncio loop to stop, joins the thread.
        """
        logger.info("Quit menu item selected. Shutting down.")

        # First, safely stop any active hotkey event taps
        logger.info("Cleaning up hotkey event taps...")
        try:
            hotkeys_stop()
            logger.info("Hotkey event taps disabled successfully")
        except Exception as e:
            logger.error(f"Error disabling hotkey event taps: {e}")

        # Stop the queue polling timer
        self.queue_timer.stop()
        logger.info("Queue timer stopped.")

        # Clean up the asyncio thread
        if self.async_loop and self.async_loop.is_running():
            logger.info("Requesting asyncio loop to stop...")
            self.async_loop.call_soon_threadsafe(self.async_loop.stop)
            # Wait for the thread to finish
            if self.async_thread:
                logger.info("Waiting for asyncio thread to join...")
                self.async_thread.join(timeout=2.0)  # Wait max 2 seconds
                if self.async_thread.is_alive():
                    logger.warning("Asyncio thread did not join cleanly.")
                else:
                    logger.info("Asyncio thread joined.")

        logger.info("Quitting rumps application.")
        rumps.quit_application()

    def _toggle_hotkeys(self, _sender):
        """Toggle hotkeys on/off based on current state.

        When toggled off, stops the event tap. When toggled on,
        attempts to restart the hotkey listeners.
        """

    # --- Mini tool helpers ---
    def _apply_cfg_env(self):
        os.environ["WHISPER_SERVER_URL"] = self.cfg.whisper_url or ""
        os.environ["LLM_SERVER_URL"] = self.cfg.llm_url or ""
        os.environ["FORMAT_ENABLED"] = "1" if self.cfg.format_enabled else "0"
        os.environ["WHISPER_LANGUAGE"] = self.cfg.language or ""
        # Reload stt/llm so they pick up new env
        try:
            import llm as llm_mod
            import stt as stt_mod

            importlib.reload(stt_mod)
            importlib.reload(llm_mod)
            # Update the imported symbols used elsewhere in this module
            global transcribe, format_text
            transcribe = stt_mod.transcribe
            format_text = llm_mod.format_text
            # Also update language state
            if self.cfg.language in (None, "", "auto"):
                stt_mod.set_language(None)
            else:
                stt_mod.set_language(self.cfg.language)
        except Exception as e:
            logger.warning(f"Reload after config apply failed: {e}")

    def _update_backend_menu_labels(self):
        # Status labels
        stt_ok = getattr(self, "_stt_ok", False)
        llm_ok = getattr(self, "_llm_ok", False)
        stt_lbl, llm_lbl = backend_status_labels(stt_ok, llm_ok)
        self.item_stt_status.title = stt_lbl
        self.item_llm_status.title = llm_lbl
        # URL / formatting labels
        for i, label in enumerate(config_labels(self.cfg)):
            if i == 0:
                # language label is already in main menu; we don't replace it here
                pass
            elif i == 1:
                # reflect formatting checkbox state
                self.menu["Enable Formatting"].state = self.cfg.format_enabled
            elif i == 2:
                self.item_w_url.title = label
            elif i == 3:
                self.item_l_url.title = label

    def _edit_whisper_url(self, _sender):
        w = rumps.Window(
            message="Enter Whisper Server URL (empty = local)",
            default_text=self.cfg.whisper_url,
            title="Configure Whisper",
            ok="Save",
            cancel="Cancel",
        )
        resp = w.run()
        if resp.clicked:
            self.cfg.whisper_url = (resp.text or "").strip()
            save_config(self.cfg)
            self._apply_cfg_env()
            # Consider re-checking backends
            self._check_backends(None)

    def _edit_llm_url(self, _sender):
        w = rumps.Window(
            message="Enter LLM Server URL (empty = local)",
            default_text=self.cfg.llm_url,
            title="Configure LLM",
            ok="Save",
            cancel="Cancel",
        )
        resp = w.run()
        if resp.clicked:
            self.cfg.llm_url = (resp.text or "").strip()
            save_config(self.cfg)
            self._apply_cfg_env()
            self._check_backends(None)

    def _check_backends(self, _sender):
        # simple sync checks with short timeout
        def _check(url: str) -> bool:
            if not url:
                return False
            try:
                r = requests.get(url.rstrip("/") + "/healthz", timeout=2)
                if r.status_code == 200:
                    j = r.json()
                    return bool(j.get("ok", False))
            except Exception:
                return False
            return False

        self._stt_ok = _check(self.cfg.whisper_url)
        self._llm_ok = _check(self.cfg.llm_url)
        self._update_backend_menu_labels()
        if self.hotkeys_enabled:
            # Disable hotkeys
            logger.info("Disabling hotkeys by user request")
            hotkeys_stop()
            self.hotkeys_enabled = False
            self.menu["Enable Hotkeys"].state = False
            self.menu["Status: Hotkeys Enabled"].title = "Status: Hotkeys Disabled"
            # Update icon to show disabled state
            MenuIcon.set(self, "🚫")
        else:
            # Enable hotkeys
            logger.info("Enabling hotkeys by user request")
            if hotkeys_start():
                self.hotkeys_enabled = True
                self.menu["Enable Hotkeys"].state = True
                self.menu["Status: Hotkeys Disabled"].title = "Status: Hotkeys Enabled"
                # Reset icon to idle state
                MenuIcon.set(self, MenuIcon.IDLE)
            else:
                # Failed to enable
                logger.error("Failed to enable hotkeys. Check accessibility permissions.")
                # Show error in menu
                self.menu["Status: Hotkeys Disabled"].title = "Status: Failed to Enable Hotkeys"
                # Show error dialog
                rumps.alert(
                    title="Hotkey Initialization Failed",
                    message=(
                        "Could not enable hotkeys. Please check Accessibility "
                        "permissions in System Settings."
                    ),
                    ok="OK",
                )

    def _toggle_formatting(self, _sender):
        """Toggle LLM formatting on/off at runtime."""
        try:
            if llm.FORMAT_ENABLED:
                llm.FORMAT_ENABLED = False
                self.menu["Enable Formatting"].state = False
                logger.info("Formatting disabled")
                # Optionally unload heavy LLM to free memory
                if os.environ.get("UNLOAD_LLM_ON_DISABLE", "0").lower() not in (
                    "0",
                    "false",
                    "no",
                    "off",
                ):
                    try:
                        llm.unload_model()
                        logger.info("LLM unloaded after disabling formatting")
                    except Exception as _e:
                        logger.warning(f"Failed to unload LLM: {_e}")
            else:
                llm.FORMAT_ENABLED = True
                self.menu["Enable Formatting"].state = True
                logger.info("Formatting enabled")
        except Exception as e:
            logger.error(f"Failed to toggle formatting: {e}")

    def _set_language_auto(self, _sender):
        """Set Whisper language to auto-detect."""
        set_language(None)
        self.menu["Language: Auto"].state = True
        self.menu["Language: Polish (PL)"].state = False
        self.menu["Language: English (EN)"].state = False
        logger.info("Language set to: auto")

    def _set_language_pl(self, _sender):
        """Force Polish transcription."""
        set_language("pl")
        self.menu["Language: Auto"].state = False
        self.menu["Language: Polish (PL)"].state = True
        self.menu["Language: English (EN)"].state = False
        logger.info("Language set to: pl")

    def _set_language_en(self, _sender):
        """Force English transcription."""
        set_language("en")
        self.menu["Language: Auto"].state = False
        self.menu["Language: Polish (PL)"].state = False
        self.menu["Language: English (EN)"].state = True
        logger.info("Language set to: en")

    def _show_toggles_help(self, _sender):
        """Show a short explanation of the toggles."""
        try:
            rumps.alert(
                title="What do these toggles do?", message=toggles_help_message("en"), ok="OK"
            )
        except Exception as e:
            logger.error(f"Failed to show toggles help: {e}")

    def _open_accessibility(self, _sender):
        """Open macOS System Settings to the Accessibility pane.

        This helps users grant the necessary permissions for hotkeys to work.
        """
        try:
            # Use AppleScript to open the Privacy & Security settings
            import subprocess

            script = (
                'tell application "System Settings"\n'
                "  activate\n"
                '  reveal anchor "Privacy_Accessibility" of pane id '
                '"com.apple.settings.PrivacySecurity.extension"\n'
                "end tell"
            )
            subprocess.run(["osascript", "-e", script])
            logger.info("Opened System Settings to Accessibility pane")
        except Exception as e:
            logger.error(f"Failed to open Accessibility settings: {e}")
            rumps.alert(
                title="Could Not Open Settings",
                message=(
                    "Please open System Settings → Privacy & Security → "
                    "Accessibility manually and enable this app."
                ),
                ok="OK",
            )

    @objc.IBAction
    def reset_(self, _sender):
        """Resets the icon to the 'idle' state.

        Called by the NSTimer scheduled in ui.menuicon.success().
        Needs to be an instance method accessible via Objective-C.

        Args:
            sender: The NSTimer object (unused, but required by selector).
        """
        logger.debug("NSTimer fired: Resetting icon to idle.")
        # Only reset to idle if hotkeys are enabled
        if self.hotkeys_enabled:
            MenuIcon.set(self, MenuIcon.IDLE)
            logger.info("UI State: Idle (🜏)")
        else:
            # Keep showing disabled state
            MenuIcon.set(self, "🚫")
            logger.info("UI State: Hotkeys disabled (🚫)")

    def run_loop(self):
        """Starts the application's main run loop and background worker thread.

        Handles hotkey initialization failures gracefully, providing user feedback
        and allowing the app to run in a degraded mode without hotkeys if needed.

        In nohup/background mode, avoids showing user dialogs that would block execution.
        """
        # Check if running in background/nohup mode
        # Multiple methods to detect background mode for robustness
        is_background_mode = False

        # Method 1: Explicit environment variable
        if os.environ.get("NOHUP_MODE", "0").lower() in ("1", "true", "yes", "on"):
            is_background_mode = True
            logger.info("Background mode detected via NOHUP_MODE environment variable")

        # Method 2: Check if stdout is redirected (common with nohup)
        try:
            if not os.isatty(sys.stdout.fileno()):
                is_background_mode = True
                logger.info("Background mode detected: stdout is not a TTY")
        except (AttributeError, OSError):
            # If we can't check isatty (could be redirected)
            pass

        # Method 3: Check parent process name (often 'nohup')
        try:
            import psutil

            try:
                parent = psutil.Process(os.getppid())
                if parent.name() in ("nohup", "daemondo", "launchd"):
                    is_background_mode = True
                    logger.info(f"Background mode detected: parent process is {parent.name()}")
            except Exception as e:
                logger.warning(f"Could not check parent process: {e}")
        except ImportError:
            # psutil not available
            logger.info("psutil not available, skipping parent process check")
            pass

        logger.info(f"Running in {'background' if is_background_mode else 'interactive'} mode")

        # Initialize hotkeys with proper error handling
        logger.info("Starting hotkey listener...")
        hotkeys_success = hotkeys_start()

        # Update UI based on hotkey initialization result
        if hotkeys_success:
            logger.info("Hotkeys initialized successfully")
            self.hotkeys_enabled = True
            self.menu["Status: Initializing..."].title = "Status: Hotkeys Enabled"
            self.menu["Enable Hotkeys"].state = True
        else:
            logger.error("Failed to initialize hotkeys. App will run without keyboard shortcuts.")
            self.hotkeys_enabled = False
            self.menu["Status: Initializing..."].title = "Status: Hotkeys Disabled"
            self.menu["Enable Hotkeys"].state = False

            # Show visual indication of disabled hotkeys
            MenuIcon.set(self, "🚫")

            # In background mode, don't show dialogs that would block execution
            if not is_background_mode:
                # Only show alert dialog when not in background mode
                try:
                    rumps.alert(
                        title="Hotkey Initialization Failed",
                        message=(
                            "Vista Scribe could not initialize keyboard shortcuts due "
                            "to missing permissions.\n\nThe app will continue to run, "
                            "but keyboard shortcuts (Ctrl hold, ⇧⌘/, double-Option) will "
                            "not work until permissions are granted.\n\nTo enable hotkeys, "
                            "click 'Open System Accessibility Settings...' in the menu, "
                            "add this app to the allowed list, then use 'Enable Hotkeys'."
                        ),
                        ok="OK",
                    )
                except Exception as e:
                    logger.error(f"Failed to show alert dialog: {e}")
            else:
                logger.info("Running in background mode - skipping alert dialogs")

        # Release event tap resources if hotkeys failed to initialize
        if not hotkeys_success:
            logger.info("Event tap stopped and resources released.")

        # Start the async thread for background processing
        logger.info("Starting asyncio worker thread...")
        self.async_thread = threading.Thread(target=self._run_async_loop, daemon=True)
        self.async_thread.start()

        # Only start queue polling if hotkeys are enabled
        if self.hotkeys_enabled:
            logger.info("Starting queue polling timer...")
            self.queue_timer.start()
        else:
            logger.info("Skipping queue polling timer (hotkeys disabled)")

        # Start the rumps application
        logger.info("Starting rumps application run loop...")
        super().run()  # blocks until quit
        logger.info("Rumps run loop finished.")

    # --- Mode handling ---
    def _refresh_mode_menu(self):
        mode = (os.environ.get("MODE", "hold") or "hold").strip().lower()
        try:
            self.menu["Mode"][0].title = "Current: " + mode.replace("_", " ")
        except Exception:
            pass
        self.item_mode_hold.state = mode == "hold"
        self.item_mode_handoff.state = mode == "hands_off"
        self.item_mode_adv.state = mode == "advanced"

    def _set_mode(self, mode: str):
        os.environ["MODE"] = mode
        try:
            update_env_vars({"MODE": mode})
        except Exception:
            pass
        self._refresh_mode_menu()
        # Friendly nudge that only anchors/UX change for now
        try:
            rumps.notification(
                title="VistaScribe",
                subtitle="Mode switched",
                message=mode.replace("_", " "),
            )
        except Exception:
            pass

    def _save_mode_env(self, _sender):
        try:
            update_env_vars({"MODE": os.environ.get("MODE", "hold")})
            rumps.notification(
                title="VistaScribe", subtitle="Mode saved", message=os.environ.get("MODE", "hold")
            )
        except Exception as e:
            logger.error(f"Failed to save MODE to .env: {e}")


# --- entry point ---

if __name__ == "__main__":
    logger.info("Application starting...")
    app = VistaScribe()
    # run_loop() handles starting the hotkey listener, worker, and rumps loop
    app.run_loop()
    logger.info("Application finished.")
