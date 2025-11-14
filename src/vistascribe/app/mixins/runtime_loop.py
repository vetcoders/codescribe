"""Mix-in wrapping run-loop / hotkey toggling utilities."""

from __future__ import annotations

import asyncio
import logging
import os
import queue
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import rumps

from ...hotkeys import start as hotkeys_start, stop as hotkeys_stop
from ...ui import MenuIcon

logger = logging.getLogger(__name__)


class RuntimeLoopMixin:
    def poll_queue(self, _timer):
        if not self.async_loop or not self.async_loop.is_running():
            return
        try:
            while not self.event_queue.empty():
                event = self.event_queue.get_nowait()
                if len(event) == 3:
                    key_type, action, is_assistive = event
                else:
                    key_type, action = event
                    is_assistive = False
                payload = {"type": key_type, "action": action, "assistive": is_assistive}
                asyncio.run_coroutine_threadsafe(
                    self.recording.handle_hotkey_event(
                        self,
                        payload,
                        beep_on_start=self.beep_on_start,
                    ),
                    self.async_loop,
                )
                self.event_queue.task_done()
        except queue.Empty:
            pass
        except Exception as exc:
            import logging

            logging.getLogger(__name__).error("Error polling queue: %s", exc, exc_info=True)

    def _run_async_loop(self):
        self.async_loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.async_loop)
        self.async_loop.run_forever()
        self.async_loop.close()

    def _quit_app(self, _sender):
        logger.info("Quit menu item selected.")
        choice = self._prompt_quit_decision()
        if choice == "cancel":
            logger.info("Quit cancelled by user.")
            return

        self._shutdown_runtime_components()

        if choice == "quit_all":
            stopped = self._stop_backend_server()
            logger.info(
                "Background server %s",
                "stopped" if stopped else "was already stopped or could not be stopped",
            )

        rumps.quit_application()

    def _focus_app_window(self, AppKit):  # pragma: no cover - macOS UI glue
        try:
            app = AppKit.NSApplication.sharedApplication()
            if app is not None:
                app.activateIgnoringOtherApps_(True)
        except Exception:
            pass
        try:
            runner = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(
                os.getpid()
            )
            if runner is not None:
                runner.activateWithOptions_(AppKit.NSApplicationActivateIgnoringOtherApps)
        except Exception:
            pass

    def _position_alert_window(self, AppKit, window):  # pragma: no cover
        try:
            screen = AppKit.NSScreen.mainScreen()
            if screen is None:
                window.center()
                return
            screen_frame = screen.visibleFrame()
            win_frame = window.frame()
            new_x = screen_frame.origin.x + (screen_frame.size.width - win_frame.size.width) / 2
            new_y = screen_frame.origin.y + (screen_frame.size.height - win_frame.size.height) / 2
            window.setFrameOrigin_((new_x, new_y))
        except Exception:
            try:
                window.center()
            except Exception:
                pass

    def _shutdown_runtime_components(self):
        try:
            self.recording.cancel_tasks()
        except Exception:
            pass
        try:
            hotkeys_stop()
        except Exception:
            pass
        try:
            self.queue_timer.stop()
        except Exception:
            pass
        if self.async_loop and self.async_loop.is_running():
            self.async_loop.call_soon_threadsafe(self.async_loop.stop)
            if self.async_thread:
                self.async_thread.join(timeout=2.0)

    def _prompt_quit_decision(self) -> str:
        """Return 'quit_all', 'tray_only', or 'cancel' based on the alert selection."""

        try:
            import AppKit
        except Exception:
            logger.debug("AppKit unavailable; defaulting to tray-only quit.")
            return "tray_only"

        alert = AppKit.NSAlert.new()
        alert.setMessageText_("Quit VistaScribe?")
        alert.setInformativeText_(
            "VistaScribe can keep the transcription server running in the background "
            "so other apps can reuse it. See README.md (Background server usage) for "
            "details. Do you want to quit the tray and the server now?"
        )
        alert.addButtonWithTitle_("Quit App & Server")
        alert.addButtonWithTitle_("Keep Background Server")
        alert.addButtonWithTitle_("Cancel")
        self._focus_app_window(AppKit)
        try:
            window = alert.window()
            if window is not None:
                window.makeKeyAndOrderFront_(None)
                window.setLevel_(AppKit.NSFloatingWindowLevel)
                self._position_alert_window(AppKit, window)
        except Exception:
            logger.debug("Could not force quit dialog to front; continuing anyway.")
        response = alert.runModal() - 1000
        if response == 0:
            return "quit_all"
        if response == 1:
            return "tray_only"
        return "cancel"

    def _stop_backend_server(self) -> bool:
        repo_root = Path(getattr(self, "repo_root", Path.cwd()))
        pid_file = repo_root / ".pids" / "vistascribe-server.pid"
        stopped = False

        if pid_file.exists():
            try:
                pid = int(pid_file.read_text().strip())
            except Exception:
                pid = None
            if pid:
                logger.info("Sending SIGTERM to VistaScribeServer (pid %s)", pid)
                try:
                    os.kill(pid, signal.SIGTERM)
                    for _ in range(20):  # wait up to ~2s
                        time.sleep(0.1)
                        try:
                            os.kill(pid, 0)
                        except OSError:
                            stopped = True
                            break
                    else:
                        logger.warning("VistaScribeServer pid %s did not exit in time", pid)
                except Exception as exc:
                    logger.warning("Failed to signal VistaScribeServer pid %s: %s", pid, exc)
            try:
                pid_file.unlink(missing_ok=True)
            except Exception:
                pass
        if stopped:
            return True

        try:
            result = subprocess.run(
                [sys.executable, "-m", "vistascribe.vistascribe_server", "stop"],
                cwd=str(repo_root),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            return result.returncode == 0
        except Exception as exc:
            logger.warning("Unable to stop VistaScribeServer via CLI: %s", exc)
            return False

    def _toggle_hotkeys(self, _sender):
        try:
            is_background = os.environ.get("NOHUP_MODE", "0").lower() in ("1", "true", "yes", "on")
            try:
                if not is_background and not os.isatty(sys.stdout.fileno()):
                    is_background = True
            except (AttributeError, ValueError, OSError):
                pass

            if getattr(self, "hotkeys_enabled", False):
                try:
                    hotkeys_stop()
                except Exception as exc:
                    logger.warning("hotkeys_stop failed: %s", exc)
                self.hotkeys_enabled = False
                self.menu["Enable Hotkeys"].state = False
                self.item_status.title = "Status: Hotkeys Disabled"
                MenuIcon.set(self, "🚫")
            else:
                ok = False
                try:
                    ok = hotkeys_start()
                except Exception as exc:
                    logger.error("hotkeys_start raised: %s", exc)
                if ok:
                    self.hotkeys_enabled = True
                    self.menu["Enable Hotkeys"].state = True
                    self.item_status.title = "Status: Hotkeys Enabled"
                    MenuIcon.set(self, MenuIcon.IDLE)
                    try:
                        if hasattr(self, "queue_timer"):
                            self.queue_timer.start()
                    except Exception:
                        pass
                else:
                    self.hotkeys_enabled = False
                    self.menu["Enable Hotkeys"].state = False
                    self.item_status.title = "Status: Failed to Enable Hotkeys"
                    MenuIcon.set(self, "🚫")
                    if not is_background:
                        try:
                            rumps.alert(
                                title="Hotkeys",
                                message=(
                                    "Could not enable hotkeys. Please grant Accessibility and "
                                    "Input Monitoring permissions, then try again."
                                ),
                                ok="OK",
                            )
                        except Exception:
                            pass
        except Exception as exc:
            logger.error("Toggle hotkeys failed: %s", exc)

    def run_loop(self):
        is_background_mode = False
        if os.environ.get("NOHUP_MODE", "0").lower() in ("1", "true", "yes", "on"):
            is_background_mode = True
        try:
            if not os.isatty(sys.stdout.fileno()):
                is_background_mode = True
        except (AttributeError, ValueError, OSError):
            pass
        try:
            import psutil

            parent = psutil.Process(os.getppid())
            if parent.name() in ("nohup", "daemondo", "launchd"):
                is_background_mode = True
        except Exception:
            pass

        hotkeys_success = hotkeys_start()
        if hotkeys_success:
            self.hotkeys_enabled = True
            self.item_status.title = "Status: Hotkeys Enabled"
            self.menu["Enable Hotkeys"].state = True
        else:
            self.hotkeys_enabled = False
            self.item_status.title = "Status: Hotkeys Disabled"
            self.menu["Enable Hotkeys"].state = False
            MenuIcon.set(self, "🚫")
            if not is_background_mode:
                try:
                    rumps.alert(
                        title="Hotkey Initialization Failed",
                        message=(
                            "Vista Scribe could not initialize keyboard shortcuts "
                            "due to missing permissions.\n\nThe app will continue to run, but "
                            "shortcuts will not work until permissions are granted."
                        ),
                        ok="OK",
                    )
                except Exception:
                    pass

        self.async_thread = threading.Thread(target=self._run_async_loop, daemon=True)
        self.async_thread.start()
        if self.hotkeys_enabled:
            self.queue_timer.start()
        super().run()
