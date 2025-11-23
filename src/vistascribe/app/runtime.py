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
import fcntl
import logging
import os
import plistlib
import subprocess
import sys
import threading
from pathlib import Path

import rumps

# Lightweight HTTP client for ML operations (keeps tray light)
from .. import (
    diag,  # developer diagnostics
    first_run,
    history,
)
from ..config import update_env_vars
from ..event_log import instrument_menu_item

# import our modules
from ..hotkeys import (
    events as hk_events,
    set_hold_mods as hotkeys_set_hold_mods,
)
from ..path_utils import repo_root
from ..permission_manager import PermissionManager
from ..ui import (
    MenuIcon,
)
from .controllers.history import HistoryController
from .controllers.models import ModelsController
from .menu_utils import create_parent_item, ensure_parent_callback, set_submenu
from .mixins.appearance import AppearanceMixin
from .mixins.backends import BackendMenuMixin
from .mixins.feedback import FeedbackMenuMixin
from .mixins.hold_menu import HoldMenuMixin
from .mixins.runtime_loop import RuntimeLoopMixin
from .mixins.tools import ToolsMixin
from .recording_controller import RecordingController

APP_ROOT = str(repo_root())
AGENT_NAME = os.environ.get("AGENT_NAME", "asystent")

# configure logging (set level for the entire application)
# consider moving this to a dedicated config area if app grows
log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
LOG_DIR = Path(repo_root()) / "logs"
LOG_DIR.mkdir(parents=True, exist_ok=True)
LOG_FILE = LOG_DIR / "VistaScribe.log"
logging.basicConfig(
    level=log_level,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    handlers=[
        logging.FileHandler(LOG_FILE, encoding="utf-8"),
        logging.StreamHandler(sys.stdout),
    ],
)
logger = logging.getLogger(__name__)


# --- singleton lock ---
def acquire_lock():
    """Acquire a lock to ensure only one instance of the application runs."""

    lock_file_path = os.path.join(APP_ROOT, ".vista_scribe.lock")

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


def _persist_env_vars(values: dict[str, str]):
    try:
        update_env_vars(values)
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)


# --- rumps application class ---


class VistaScribe(
    AppearanceMixin,
    BackendMenuMixin,
    FeedbackMenuMixin,
    HoldMenuMixin,
    ToolsMixin,
    RuntimeLoopMixin,
    PermissionManager,
    rumps.App,
):
    """macOS menu bar application class using rumps.

    Integrates hotkey listening, state management, and UI updates.
    Runs asyncio operations in a separate thread.
    """

    def __init__(self):
        """Initializes the rumps app, queue timer, and asyncio thread setup."""
        super().__init__(MenuIcon.IDLE, quit_button=None)

        self.beep_on_start = os.environ.get("BEEP_ON_START", "1").strip().lower() not in {
            "0",
            "false",
            "no",
            "off",
        }
        self.show_tray_glyph = os.environ.get("SHOW_TRAY_GLYPH", "1").strip().lower() not in {
            "0",
            "false",
            "no",
            "off",
        }
        self.history_enabled = os.environ.get("HISTORY_ENABLED", "1").strip().lower() not in {
            "0",
            "false",
            "no",
            "off",
        }
        self.recording = RecordingController()

        # Try to set a tray icon image (keeps state glyphs out of the title area)
        try:
            from ..path_utils import normalize_model_path  # lazy import to avoid cycles

            icon_env = os.environ.get("TRAY_ICON")
            repo_root = APP_ROOT
            pkg_root = str(Path(__file__).resolve().parent)
            icon_candidates = [
                os.path.join(pkg_root, "assets", "icon.png"),
                os.path.join(repo_root, "assets", "icon.png"),
                os.path.join(repo_root, "src", "vistascribe", "assets", "icon.png"),
            ]
            default_icon = next(
                (c for c in icon_candidates if os.path.exists(c)), icon_candidates[0]
            )
            candidate = icon_env or default_icon
            if candidate:
                norm = normalize_model_path(candidate) or candidate
                if os.path.isfile(norm):
                    self.template = True
                    self.icon = norm
                    if self.show_tray_glyph:
                        self.title = MenuIcon.IDLE
                    else:
                        self.title = ""
                    logger.info(f"Tray icon set: {norm}")
                    if os.environ.get("DEV_MODE", "0").lower() in ("1", "true", "yes", "on"):
                        MenuIcon.set(self, MenuIcon.IDLE)
        except Exception as e:
            logger.warning(f"Tray icon setup skipped: {e}")

        self.repo_root = APP_ROOT

        # Initialize menu with app status and controls
        self.hotkeys_enabled = True  # Default state, will be updated in run_loop
        self.item_status = rumps.MenuItem("Status: Initializing...", callback=lambda _s: None)

        # Hotkey settings submenu
        self._init_hold_menu()

        # Language + formatting submenus live alongside the main controls
        self._init_language_menu()
        self.menu_formatting = create_parent_item("Formatting")
        try:
            self._refresh_formatting_menu()
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

        # Appearance submenu (tray icon / glyph options)
        self.item_tray_glyph = rumps.MenuItem(
            "Show status glyph next to icon",
            callback=self._toggle_tray_glyph,
        )
        self.item_refresh_tray = rumps.MenuItem(
            "Refresh Tray Icon", callback=lambda _s: self._refresh_tray_icon()
        )
        self.menu_appearance = create_parent_item("Appearance")
        self._rebuild_appearance_menu()

        # Feedback (start sound) submenu
        self._init_feedback_menu()

        # History submenu
        self.history = HistoryController(self)
        self.menu_history = self.history.menu

        self.models = ModelsController(self)
        self.menu_models = self.models.menu

        # --- Mini config tool (Backends) ---
        self._init_backend_menu()

        # Permissions submenu        # Permissions submenu
        self._perm_snapshot: dict | None = None
        self.item_perm_status = rumps.MenuItem("Status: Pending", callback=lambda _s: None)
        self.item_perm_check = rumps.MenuItem(
            "Check Permissions Now", callback=self._check_permissions
        )
        self.item_perm_prompt_ax = rumps.MenuItem(
            "Request Accessibility Prompt", callback=self._prompt_accessibility_permission
        )
        self.item_perm_request_mic = rumps.MenuItem(
            "Request Microphone Access", callback=self._request_microphone_access
        )
        self.item_perm_open_access = rumps.MenuItem(
            "Open Accessibility Settings", callback=self._open_accessibility
        )
        self.item_perm_open_input = rumps.MenuItem(
            "Open Input Monitoring Settings", callback=self._open_input_monitoring_settings
        )
        self.item_perm_open_mic = rumps.MenuItem(
            "Open Microphone Settings", callback=self._open_microphone_settings
        )
        self.menu_permissions = create_parent_item("Permissions")
        self._rebuild_permissions_menu()

        # Tools submenu
        self._init_tools_menu()

        # Build submenus before attaching to the status bar so rumps sees them immediately.
        self._refresh_language_submenu()
        self._refresh_formatting_menu()
        self._rebuild_hold_menu()
        self.models.refresh()
        self._rebuild_backend_menu()
        self.history.refresh()
        self._rebuild_appearance_menu()
        self._refresh_feedback_menu()
        self._rebuild_permissions_menu()
        self._rebuild_tools_menu()

        # Assemble top-level menu
        self.menu = [
            self.item_status,
            None,
            "Enable Hotkeys",
            None,
            self.menu_language,
            self.menu_formatting,
            None,
            self.menu_hotkeys,
            None,
            self.menu_models,
            self.menu_backends,
            None,
            self.menu_history,
            None,
            self.menu_appearance,
            self.menu_feedback,
            None,
            self.menu_permissions,
            self.menu_tools,
            None,
            "What do these toggles do?",
            None,
            "Start at Login",
            None,
            "Quit...",
        ]

        # Set callbacks for top-level string items
        self.menu["Enable Hotkeys"].set_callback(self._toggle_hotkeys)
        instrument_menu_item(self.menu["Enable Hotkeys"])
        self.menu["What do these toggles do?"].set_callback(self._show_toggles_help)
        instrument_menu_item(self.menu["What do these toggles do?"])
        self.menu["Start at Login"].set_callback(self._toggle_login_item)
        instrument_menu_item(self.menu["Start at Login"])
        self.menu["Quit..."].set_callback(self._quit_app)
        instrument_menu_item(self.menu["Quit..."])

        # Ensure submenus reflect current state after settings/env load
        self._refresh_hold_menu()
        self._refresh_appearance_menu()
        self._refresh_feedback_menu()
        self._refresh_history_menu()
        self.models.refresh()
        self._rebuild_tools_menu()

        # Populate models/backends/permissions after helpers initialise
        self._apply_cfg_env()
        self._update_backend_menu_labels()
        self._update_permissions_menu()

        # Optional: dump menu tree on startup for diagnostics
        try:
            if os.environ.get("DUMP_MENU_TREE", "0").lower() in ("1", "true", "yes", "on"):
                self._export_menu_tree(None)
                # Exit after exporting to avoid running the UI loop
                os._exit(0)
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

        # Minimal first-run setup: config + sensible defaults (non-blocking via callAfter)
        if getattr(rumps, "AppHelper", None):
            rumps.AppHelper.callAfter(first_run.ensure_config_and_permissions)
        else:
            # Fallback for CLI mode or non-macOS
            try:
                first_run.ensure_config_and_permissions()
            except Exception as e:
                logger.warning(f"First-run setup skipped: {e}")

        # Disable menu items initially until we know hotkeys status
        self.menu["Enable Hotkeys"].state = False
        # Language state is now handled in the Language submenu

        self.event_queue = hk_events()  # get the standard queue
        self.async_loop = None
        self.async_thread = None
        # Polling interval for hotkey/event queue; configurable to trade CPU vs. latency.
        queue_poll_interval = float(os.environ.get("QUEUE_POLL_INTERVAL", "0.02"))
        self.queue_timer = rumps.Timer(self.poll_queue, queue_poll_interval)
        self._latest_history_path: Path | None = None
        logger.info("Vista Scribe App initialized.")
        # Developer diagnostics: preflight snapshot if DEV_MODE enabled
        try:
            if os.environ.get("DEV_MODE", "0").lower() in ("1", "true", "yes", "on"):
                info = diag.run_preflight(logger)
                diag.write_snapshot(info, APP_ROOT)
        except Exception as e:
            logger.debug(f"Preflight snapshot failed: {e}")

        # Ensure Tools menu works even if backend not running
        try:
            self.item_open_lab.set_callback(self._open_voice_chat_lab)
            instrument_menu_item(self.item_open_lab)
            self.item_export_menu.set_callback(self._export_menu_tree)
            instrument_menu_item(self.item_export_menu)
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

        # Reflect Start at Login state
        try:
            self.menu["Start at Login"].state = self._is_login_installed()
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

        # Background permission probe (non-blocking)
        self._schedule_permission_probe()
        self._schedule_menu_health_check()
        self._menu_health_check()

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
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

    def _show_toggles_help(self, _sender):
        from ..ui import toggles_help_message

        try:
            rumps.alert(title="VistaScribe", message=toggles_help_message())
        except Exception as exc:
            logger.debug("Unable to show toggles help: %s", exc)

    def _rebuild_appearance_menu(self) -> None:
        set_submenu(
            self.menu_appearance,
            [self.item_tray_glyph, None, self.item_refresh_tray],
        )

    def _rebuild_permissions_menu(self) -> None:
        set_submenu(
            self.menu_permissions,
            [
                self.item_perm_status,
                None,
                self.item_perm_check,
                self.item_perm_prompt_ax,
                self.item_perm_request_mic,
                None,
                self.item_perm_open_access,
                self.item_perm_open_input,
                self.item_perm_open_mic,
            ],
        )

    def _refresh_formatting_menu(self) -> None:
        try:
            from ..menu_formatting import build_formatting_menu

            build_formatting_menu(self, self.menu_formatting)
        except Exception as exc:
            logger.debug("Failed to refresh formatting menu: %s", exc)

    def _schedule_menu_health_check(self) -> None:
        def _runner() -> None:
            helper = getattr(rumps, "AppHelper", None)
            if helper is not None:
                helper.call_after(self._menu_health_check)
            else:
                self._menu_health_check()

        timer = threading.Timer(0.3, _runner)
        timer.daemon = True
        timer.start()

    def _menu_health_targets(self):
        return [
            ("Language", self.menu_language, self._refresh_language_submenu),
            ("Formatting", self.menu_formatting, self._refresh_formatting_menu),
            (
                "Hold Hotkeys",
                self.menu_hotkeys,
                getattr(self, "_rebuild_hold_menu", self._refresh_hold_menu),
            ),
            ("Models", self.menu_models, self.models.refresh),
            ("Backends", self.menu_backends, self._rebuild_backend_menu),
            ("History", self.menu_history, self.history.refresh),
            ("Appearance", self.menu_appearance, self._rebuild_appearance_menu),
            (
                "Feedback",
                self.menu_feedback,
                getattr(self, "_rebuild_feedback_menu", self._refresh_feedback_menu),
            ),
            ("Permissions", self.menu_permissions, self._rebuild_permissions_menu),
            (
                "Tools",
                self.menu_tools,
                getattr(self, "_rebuild_tools_menu", lambda: None),
            ),
        ]

    def _menu_health_check(self) -> None:
        for label, menu_item, rebuild in self._menu_health_targets():
            ensure_parent_callback(menu_item)
            backing = getattr(menu_item, "_menu", None)
            needs_rebuild = backing is None
            if not needs_rebuild:
                try:
                    children = list(menu_item)
                except Exception:
                    children = []
                needs_rebuild = len(children) == 0
            if not needs_rebuild:
                continue
            try:
                rebuild()
                logger.warning("Menu health: rebuilt %s submenu (empty or missing)", label)
            except Exception as exc:
                logger.warning("Menu health: failed to rebuild %s submenu: %s", label, exc)

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

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        subprocess.run(["launchctl", "load", "-w", path], check=False)
        logger.info("Installed Start at Login LaunchAgent")

    def _remove_login_agent(self):
        path = self._login_plist_path()

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        try:
            os.remove(path)
        except FileNotFoundError:
            logger.debug("LaunchAgent plist already removed: %s", path)
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
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
                self._refresh_hold_menu()

            # Step 2: Toggle trigger
            alert2 = AppKit.NSAlert.new()
            alert2.setMessageText_("Toggle Trigger")
            alert2.setInformativeText_("Choose the hands‑off trigger.")
            trig_map = [
                ("Double Option", "double_option"),
                ("Double Right‑Option", "double_ralt"),
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
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
                os.environ["TOGGLE_TRIGGER"] = trig
                try:
                    update_env_vars({"TOGGLE_TRIGGER": trig})
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
                try:
                    self.item_toggle_current.title = "Toggle: " + self._toggle_label(trig)
                except Exception:
                    self.item_toggle_current.title = "Toggle: " + trig.replace("_", " ")
        except Exception as e:
            logger.error(f"Configure hotkeys failed: {e}")

    # --- Models management ---
    def _schedule_history_refresh(self):
        self.history.schedule_refresh()

    def _refresh_history_menu(self):
        self.history.refresh()

    def _open_history_folder(self):
        history.open_history_folder()

    def _toggle_history_enabled(self):
        self.history.toggle_history()

    def _copy_history_entry(self, path: Path):
        self.history.copy_entry(path)

    def _copy_latest_history(self):
        self.history.copy_latest()

    def _archive_transcript(self, text: str):
        self.history.archive_transcript(text)

    # --- Permissions helpers ---


# --- entry point helpers ---


def _set_process_title() -> None:
    """Try to label the process for Activity Monitor visibility."""

    try:
        import setproctitle

        setproctitle.setproctitle("VistaScribeTray")
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)  # Optional dependency


def run() -> None:
    """Start the tray app with singleton guarding."""

    _set_process_title()

    lock_handle = acquire_lock()
    if lock_handle is None:
        print("ERROR: Another instance of VistaScribe is already running.")
        print("Stop it first with: ./VistaScribe stop")
        sys.exit(1)

    logger.info("Application starting...")
    app = VistaScribe()
    app.run_loop()
    logger.info("Application finished.")


if __name__ == "__main__":
    run()
