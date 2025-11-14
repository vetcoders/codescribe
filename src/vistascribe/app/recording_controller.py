"""Recording pipeline + hotkey state machine extracted from runtime."""

from __future__ import annotations

import asyncio
import logging
import os
from typing import Any

import rumps

from .. import client, history, stats
from ..audio import Recorder
from ..stt import get_language
from ..ui import (
    MenuIcon,
    focused_element_accepts_text,
    hide_hold_badge,
    paste_text,
    show_hold_badge,
    start_sound,
)
from .status import set_status

try:  # optional developer feature
    from ..codescribe_context import save_to_codescribe

    HAS_CODESCRIBE = True
except ImportError:  # pragma: no cover - optional dependency
    HAS_CODESCRIBE = False
    save_to_codescribe = None

logger = logging.getLogger(__name__)


def _env_bool(name: str, default: str = "1") -> bool:
    return (os.environ.get(name, default) or "").strip().lower() not in {
        "0",
        "false",
        "no",
        "off",
    }


class RecordingController:
    """Encapsulates recorder lifecycle + hotkey-driven state machine."""

    def __init__(self) -> None:
        self.recorder = Recorder()
        self.state = "IDLE"
        self.assistive_mode = False
        self._hold_start_task: asyncio.Task | None = None
        self._live_stream_task: asyncio.Task | None = None
        self._hold_streaming = _env_bool("HOLD_STREAMING", "1")
        self._hold_stream_interval_ms = int(
            os.environ.get("HOLD_STREAM_INTERVAL_MS", "1000") or 1000
        )
        # Users frequently tap Ctrl accidentally, so we require a longer default
        # dwell (800 ms) before the recorder actually spins up. The value remains
        # configurable via CTRL_HOLD_DELAY_MS / HOLD_START_DELAY_MS for power users.
        self._hold_start_delay_ms = int(
            os.environ.get(
                "CTRL_HOLD_DELAY_MS",
                os.environ.get("HOLD_START_DELAY_MS", "800"),
            )
            or 800
        )

    # ------------------------------------------------------------------
    # Public helpers
    # ------------------------------------------------------------------
    def current_state(self) -> str:
        return self.state

    def reset_tray_icon(self, app: rumps.App) -> None:
        glyph = MenuIcon.IDLE
        if self.state in {"REC_HOLD", "REC_TOGGLE"}:
            glyph = MenuIcon.LISTEN
        elif self.state == "BUSY":
            glyph = MenuIcon.THINK
        MenuIcon.set(app, glyph)

    def cancel_tasks(self) -> None:
        if self._hold_start_task and not self._hold_start_task.done():
            self._hold_start_task.cancel()
        self._hold_start_task = None
        if self._live_stream_task and not self._live_stream_task.done():
            self._live_stream_task.cancel()
        self._live_stream_task = None

    # ------------------------------------------------------------------
    # Core pipeline
    # ------------------------------------------------------------------
    async def finish_recording(self, app: rumps.App) -> None:
        """Stop recording, run STT + formatting, and paste/archive the result."""

        if self.state in {"IDLE", "BUSY"}:
            logger.warning("finish_recording called while state=%s; ignoring", self.state)
            return

        logger.info("Finishing recording (state=%s)", self.state)
        logger.debug("STATE TRANSITION: %s → BUSY", self.state)
        previous_state = self.state
        self.state = "BUSY"
        MenuIcon.think(app)
        set_status(app, "Processing…")

        try:
            path = await self.recorder.stop()
            if not path:
                logger.error("Audio recording failed or produced no file.")
                MenuIcon.set(app, MenuIcon.IDLE)
                set_status(app, "Ready")
                self.state = "IDLE"
                self.assistive_mode = False
                return

            logger.info("Transcribing audio file: %s", path)
            if not client.start_server_if_needed():
                logger.error("Failed to start VistaScribeServer")
                MenuIcon.set(app, MenuIcon.IDLE)
                set_status(app, "Ready")
                self.state = "IDLE"
                self.assistive_mode = False
                return

            raw_text = await client.transcribe_http(path, language=get_language())
            if not raw_text:
                logger.error("Transcription failed: no text returned")
                MenuIcon.set(app, MenuIcon.IDLE)
                set_status(app, "Ready")
                self.state = "IDLE"
                self.assistive_mode = False
                if os.path.exists(path):
                    try:
                        os.remove(path)
                    except OSError as exc:  # pragma: no cover - best effort cleanup
                        logger.error("Failed to remove temp file %s: %s", path, exc)
                return
            raw_text = raw_text.strip()
            logger.debug("Raw transcript captured (%d chars)", len(raw_text or ""))

            logger.info("Formatting transcript (assistive=%s)…", self.assistive_mode)
            formatted_text = await client.format_text_http(raw_text, assistive=self.assistive_mode)
            text_to_paste = formatted_text if formatted_text else raw_text
            logger.debug(
                "Formatted transcript ready (%d chars, assistive=%s)",
                len(text_to_paste or ""),
                self.assistive_mode,
            )

            formatted_for_stats = formatted_text if formatted_text is not None else raw_text
            try:
                stats.record_transcript(
                    raw_text,
                    formatted_for_stats,
                    getattr(self.recorder, "last_duration", 0.0),
                )
            except Exception:  # pragma: no cover - telemetry is best effort
                logger.debug("Failed to record telemetry stats", exc_info=True)

            if HAS_CODESCRIBE and save_to_codescribe:
                try:
                    save_to_codescribe(
                        raw_text, formatted_text or raw_text, assistive=self.assistive_mode
                    )
                except Exception as exc:  # pragma: no cover
                    logger.debug("Failed to save to .codescribe: %s", exc)

            if text_to_paste:
                if focused_element_accepts_text():
                    paste_text(text_to_paste)
                    if getattr(app, "history_enabled", False):
                        entry = history.save_entry(text_to_paste)
                        app._latest_history_path = entry.path
                        app._schedule_history_refresh()
                else:
                    logger.info(
                        "No editable field detected; archiving transcript instead of pasting."
                    )
                    app._archive_transcript(text_to_paste)
            else:
                logger.warning("No text available to paste after processing.")

            MenuIcon.success(app)
            self.state = "IDLE"
            self.assistive_mode = False
            set_status(app, "Ready")
            logger.info("Processing finished successfully. State reset to IDLE.")

            if self._live_stream_task and not self._live_stream_task.done():
                self._live_stream_task.cancel()
        except Exception as exc:
            logger.error("Unexpected error during finish_recording: %s", exc, exc_info=True)
            MenuIcon.set(app, MenuIcon.IDLE)
            set_status(app, "Ready")
            self.state = "IDLE"
            self.assistive_mode = False
        finally:
            if "path" in locals() and path and os.path.exists(path):
                try:
                    os.remove(path)
                except OSError as exc:  # pragma: no cover
                    logger.error("Failed to remove temp file %s: %s", path, exc)
            try:
                hide_hold_badge()
            except Exception:  # pragma: no cover
                pass

    # ------------------------------------------------------------------
    # Live streaming helpers
    # ------------------------------------------------------------------
    async def _live_transcribe_loop(self, app: rumps.App) -> None:
        try:
            iteration = 0
            while self.state == "REC_HOLD":
                iteration += 1
                min_sec = max(0.6, self._hold_stream_interval_ms / 1000.0 * 0.6)
                try:
                    path = self.recorder.snapshot_wav(min_seconds=min_sec)
                except Exception as exc:
                    logger.debug("Live snapshot failed: %s", exc)
                    path = None
                if path:
                    try:
                        result = await client.transcribe_http(path, language=get_language())
                        txt = (result or "").strip()
                    except Exception:
                        txt = None
                    try:
                        if path and os.path.exists(path):
                            os.remove(path)
                    except OSError:
                        pass
                    if txt:
                        preview = txt.splitlines()[0][:60]
                        set_status(app, f"Listening… {preview}")
                await asyncio.sleep(self._hold_stream_interval_ms / 1000)
        except asyncio.CancelledError:  # pragma: no cover
            pass
        except Exception as exc:
            logger.debug("Live loop crashed: %s", exc)

    async def _start_recording_after_delay(self, app: rumps.App, *, beep_on_start: bool) -> None:
        try:
            await asyncio.sleep(self._hold_start_delay_ms / 1000)
            if self.state != "IDLE":
                return
            if not focused_element_accepts_text():
                logger.info("Hold ignored: focused element not editable")
                return
            if beep_on_start:
                start_sound()
            await self.recorder.start()
            MenuIcon.listen(app)
            set_status(app, "Listening…")
            try:
                show_hold_badge()
            except Exception:
                pass
            self.state = "REC_HOLD"
            if self._hold_streaming:
                self._live_stream_task = asyncio.create_task(self._live_transcribe_loop(app))
        except asyncio.CancelledError:  # pragma: no cover
            pass
        except Exception as exc:
            logger.error("Hold-start delayed task failed: %s", exc, exc_info=True)
            MenuIcon.set(app, MenuIcon.IDLE)
            self.state = "IDLE"

    # ------------------------------------------------------------------
    # Hotkey entry point
    # ------------------------------------------------------------------
    async def handle_hotkey_event(
        self,
        app: rumps.App,
        event: dict[str, Any],
        *,
        beep_on_start: bool,
    ) -> None:
        key_type = event.get("type")
        action = event.get("action")
        is_assistive = event.get("assistive", False)

        if action == "down":
            self.assistive_mode = is_assistive
        elif action == "press":
            self.assistive_mode = is_assistive

        logger.debug(
            "Hotkey event: type=%s action=%s assistive=%s state=%s",
            key_type,
            action,
            self.assistive_mode,
            self.state,
        )

        if self.state == "BUSY":
            logger.info("App busy; ignoring hotkey event")
            return

        if key_type == "hold":
            await self._handle_hold_event(app, action, beep_on_start)
        elif key_type == "toggle" and action == "press":
            await self._handle_toggle_event(app, beep_on_start)

    async def _handle_hold_event(self, app: rumps.App, action: str, beep_on_start: bool) -> None:
        if action == "down" and self.state == "IDLE":
            if not focused_element_accepts_text():
                logger.info("Hold ignored: focused element not editable")
                return
            if self._hold_start_task and not self._hold_start_task.done():
                self._hold_start_task.cancel()
            self._hold_start_task = asyncio.create_task(
                self._start_recording_after_delay(app, beep_on_start=beep_on_start)
            )
        elif action == "up":
            if self.state == "REC_HOLD":
                logger.info("Hold released; finishing recording")
                await self.finish_recording(app)
            else:
                if self._hold_start_task and not self._hold_start_task.done():
                    self._hold_start_task.cancel()
                    try:
                        await self._hold_start_task
                    except Exception:
                        pass
                self._hold_start_task = None

    async def _handle_toggle_event(self, app: rumps.App, beep_on_start: bool) -> None:
        if self.state == "IDLE":
            if not focused_element_accepts_text():
                logger.info("Toggle ignored: focused element not editable")
                return
            try:
                await self.recorder.start()
                MenuIcon.listen(app)
                set_status(app, "Listening…")
                if beep_on_start:
                    start_sound()
                try:
                    show_hold_badge()
                except Exception:
                    pass
                self.state = "REC_TOGGLE"
            except Exception as exc:
                logger.error("Failed to start recording on toggle: %s", exc, exc_info=True)
                MenuIcon.set(app, MenuIcon.IDLE)
                self.state = "IDLE"
                if getattr(self.recorder, "_stream", None):
                    try:
                        await self.recorder.stop()
                    except Exception as stop_exc:
                        logger.error("Cleanup recorder.stop failed: %s", stop_exc)
        elif self.state == "REC_TOGGLE":
            if self._live_stream_task and not self._live_stream_task.done():
                self._live_stream_task.cancel()
            await self.finish_recording(app)
