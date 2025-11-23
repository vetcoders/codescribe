"Recording pipeline + hotkey state machine extracted from runtime."

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
import time
import uuid
from typing import Any

import rumps

from .. import client, history, stats
from ..audio import Recorder
from ..path_utils import user_data_root
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
    from ..codescribe_context import save_to_codescribe as _save_to_codescribe_impl

    HAS_CODESCRIBE = True
    save_to_codescribe = _save_to_codescribe_impl
except ImportError:  # pragma: no cover - optional dependency
    HAS_CODESCRIBE = False
    save_to_codescribe = None  # type: ignore

logger = logging.getLogger(__name__)


def _env_bool(name: str, default: str = "1") -> bool:
    return (os.environ.get(name, default) or "").strip().lower() not in {
        "0",
        "false",
        "no",
        "off",
    }


MIN_TRANSCRIPT_CHARS = 32
MIN_RECORDING_DURATION_SEC = 8.0


class RecordingController:
    """Encapsulates recorder lifecycle + hotkey-driven state machine."""

    def __init__(self) -> None:
        self.recorder = Recorder()
        self.state = "IDLE"
        self.assistive_mode = False
        self._hold_start_task: asyncio.Task | None = None
        self._live_stream_task: asyncio.Task | None = None
        self._session_id: str | None = None
        self._serial_lock = asyncio.Lock()
        self._dump_audio_logs = _env_bool("DUMP_AUDIO_LOGS", "0")
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

        # Clean up stale recordings on startup
        self._cleanup_recordings()

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
        self._session_id = None

    def _recordings_dir(self) -> str:
        path = user_data_root() / "recordings"
        path.mkdir(parents=True, exist_ok=True)
        return str(path)

    def _cleanup_recordings(self) -> None:
        """Remove recordings older than retention window."""

        retention_min = int(os.environ.get("FALLBACK_RETENTION_MINUTES", "60") or 60)
        cutoff = time.time() - retention_min * 60
        try:
            base = self._recordings_dir()
            for entry in os.scandir(base):
                try:
                    if entry.is_file() and entry.stat().st_mtime < cutoff:
                        os.remove(entry.path)
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
        except Exception:
            logger.debug("cleanup_recordings failed", exc_info=True)

    def _persist_recording(self, src_path: str, reason: str) -> None:
        """Copy the temp WAV into the fallback recordings directory."""

        try:
            base = self._recordings_dir()
            ts = int(time.time() * 1000)
            sid = self._session_id or "nosession"
            name = f"{ts}_{sid}_{reason}.wav"
            dest = os.path.join(base, name)
            shutil.copy2(src_path, dest)
            logger.info("Saved fallback recording: %s (reason=%s)", dest, reason)
            self._cleanup_recordings()
        except Exception as exc:
            logger.debug("Failed to save fallback recording: %s", exc, exc_info=True)

    async def _cancel_pending_hold_start(self) -> None:
        """Abort any delayed hold-start task to avoid overlapping start/stop."""

        if self._hold_start_task and not self._hold_start_task.done():
            self._hold_start_task.cancel()
            try:
                await self._hold_start_task
            except Exception as exc:
                logger.debug("Suppressed exception", exc_info=exc)
        self._hold_start_task = None

    def _emit_event(self, event: str, **fields: Any) -> None:
        """Write a JSONL audit event to user data logs."""

        try:
            payload = {
                "ts": time.time(),
                "event": event,
                "state": self.state,
                "session_id": self._session_id,
            }
            for k, v in fields.items():
                if v is None:
                    continue
                payload[k] = v
            dest = user_data_root() / "logs" / "client.log"
            dest.parent.mkdir(parents=True, exist_ok=True)
            with dest.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        except Exception:
            logger.debug("Failed to write audit event", exc_info=True)

    def _maybe_dump_audio(self, src_path: str, reason: str) -> None:
        """Copy raw audio to user data logs/audio when the dev flag is on."""

        if not self._dump_audio_logs or not src_path:
            return
        try:
            dest_dir = user_data_root() / "logs" / "audio"
            dest_dir.mkdir(parents=True, exist_ok=True)
            ts = int(time.time() * 1000)
            sid = self._session_id or "nosession"
            dest = dest_dir / f"{ts}_{sid}_{reason}.wav"
            shutil.copy2(src_path, dest)
            logger.info("Raw audio dumped for debugging: %s", dest)
        except Exception:
            logger.debug("Failed to dump raw audio", exc_info=True)

    def _log_recording_metrics(
        self,
        *,
        response_len: int,
        formatted_len: int | None,
    ) -> None:
        diag = getattr(self.recorder, "diagnostics", None)
        if not diag:
            return
        logger.info(
            "Recording metrics: duration=%.3fs frames=%s bytes=%s chunks=%s "
            "snapshot_frames=%s snapshot_bytes=%s response_chars=%s formatted_chars=%s sid=%s",
            diag.duration_sec,
            diag.frames,
            diag.bytes,
            diag.chunks,
            diag.snapshot_frames,
            diag.snapshot_bytes,
            response_len,
            formatted_len if formatted_len is not None else 0,
            self._session_id,
        )

    # ------------------------------------------------------------------
    # Core pipeline
    # ------------------------------------------------------------------
    async def finish_recording(self, app: rumps.App) -> None:
        """Stop recording, run STT + formatting, and paste/archive the result."""

        await self._cancel_pending_hold_start()
        async with self._serial_lock:
            await self._finish_recording_locked(app)

    async def _finish_recording_locked(self, app: rumps.App) -> None:
        """Stop recording, run STT + formatting, and paste/archive the result (serialized)."""

        if self.state in {"IDLE", "BUSY"}:
            logger.warning(
                "finish_recording called while state=%s; ignoring. (Race?)",
                self.state,
            )
            return

        logger.info("Finishing recording (state=%s)", self.state)
        self._emit_event("finish_requested", mode=self.state)
        logger.debug("STATE TRANSITION: %s → BUSY", self.state)
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
                self._session_id = None
                self._emit_event("recording_failed", reason="no_path")
                return

            logger.info("Transcribing audio file: %s", path)
            self._maybe_dump_audio(path, "session")
            if not client.start_server_if_needed():
                logger.error("Failed to start VistaScribeServer")
                MenuIcon.set(app, MenuIcon.IDLE)
                set_status(app, "Ready")
                self.state = "IDLE"
                self.assistive_mode = False
                self._session_id = None
                self._emit_event("recording_failed", reason="backend_unavailable")
                return

            duration_sec = getattr(self.recorder, "last_duration", 0.0)
            raw_text = await client.transcribe_http(
                path, language=get_language(), session_id=self._session_id
            )
            raw_text = (raw_text or "").strip()
            if not raw_text:
                self._log_recording_metrics(response_len=0, formatted_len=None)
                logger.error("Transcription failed: no text returned")
                self._emit_event(
                    "recording_failed",
                    reason="empty_transcript",
                    duration_sec=duration_sec,
                )
                try:
                    self._persist_recording(path, "transcribe_failed")
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
                MenuIcon.set(app, MenuIcon.IDLE)
                set_status(app, "Ready")
                self.state = "IDLE"
                self.assistive_mode = False
                self._session_id = None
                if os.path.exists(path):
                    try:
                        os.remove(path)
                    except OSError as exc:  # pragma: no cover - best effort cleanup
                        logger.error("Failed to remove temp file %s: %s", path, exc)
                return
            logger.debug("Raw transcript captured (%d chars)", len(raw_text or ""))
            is_truncated = (
                len(raw_text) < MIN_TRANSCRIPT_CHARS and duration_sec >= MIN_RECORDING_DURATION_SEC
            )

            logger.info("Formatting transcript (assistive=%s)…", self.assistive_mode)
            formatted_text = await client.format_text_http(
                raw_text, assistive=self.assistive_mode, session_id=self._session_id
            )
            text_to_paste = formatted_text if formatted_text else raw_text
            self._log_recording_metrics(
                response_len=len(raw_text),
                formatted_len=len(formatted_text) if formatted_text else None,
            )
            logger.debug(
                "Formatted transcript ready (%d chars, assistive=%s)",
                len(text_to_paste or ""),
                self.assistive_mode,
            )
            self._emit_event(
                "recording_finished",
                duration_sec=duration_sec,
                raw_chars=len(raw_text),
                formatted_chars=len(formatted_text) if formatted_text else None,
                truncated=is_truncated,
            )
            if is_truncated:
                try:
                    self._persist_recording(path, "truncated_output")
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)

            formatted_for_stats = formatted_text if formatted_text is not None else raw_text
            try:
                stats.record_transcript(
                    raw_text,
                    formatted_for_stats,
                    getattr(self.recorder, "last_duration", 0.0),
                )
            except Exception:  # pragma: no cover - telemetry is best effort
                logger.debug("Failed to record telemetry stats", exc_info=True)

            if HAS_CODESCRIBE and save_to_codescribe is not None:
                try:
                    save_to_codescribe(
                        raw_text, formatted_text or raw_text, assistive=self.assistive_mode
                    )
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)

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
            self._session_id = None
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
            self._session_id = None
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
            # Ensure backend is up before attempting any live snapshots
            if not client.check_server_status().get("server"):
                if not client.start_server_if_needed():
                    logger.warning("Live streaming disabled: VistaScribeServer unavailable")
                    return
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
                        result = await client.transcribe_http(
                            path, language=get_language(), session_id=self._session_id
                        )
                        txt = (result or "").strip()
                        if not txt:
                            try:
                                self._persist_recording(path, "stream_transcribe_failed")
                            except Exception as exc:
                                logger.debug("Suppressed exception", exc_info=exc)
                        elif len(txt) < 32:
                            try:
                                self._persist_recording(path, "stream_truncated_output")
                            except Exception as exc:
                                logger.debug("Suppressed exception", exc_info=exc)
                    except Exception as exc:
                        logger.debug("Snapshot parse failed", exc_info=exc)
                        txt = None
                    try:
                        if path and os.path.exists(path):
                            os.remove(path)
                    except OSError as exc:
                        logger.debug("Failed to remove temp snapshot: %s", exc, exc_info=exc)
                    if txt:
                        preview = txt.splitlines()[0][:60]
                        set_status(app, f"Listening… {preview}")
                        diag = getattr(self.recorder, "diagnostics", None)
                        self._emit_event(
                            "live_snapshot",
                            iteration=iteration,
                            preview_chars=len(txt),
                            snapshot_frames=diag.snapshot_frames if diag else None,
                            snapshot_bytes=diag.snapshot_bytes if diag else None,
                        )
                await asyncio.sleep(self._hold_stream_interval_ms / 1000)
        except asyncio.CancelledError:  # pragma: no cover
            pass
        except Exception as exc:
            logger.debug("Live loop crashed: %s", exc)

    async def _start_recording_after_delay(self, app: rumps.App, *, beep_on_start: bool) -> None:
        try:
            await asyncio.sleep(self._hold_start_delay_ms / 1000)
            async with self._serial_lock:
                if self.state != "IDLE":
                    return
                if not self._session_id:
                    self._session_id = uuid.uuid4().hex
                if not client.start_server_if_needed():
                    logger.error("Hold-start aborted: VistaScribeServer unavailable")
                    return
                if beep_on_start:
                    start_sound()
                await self.recorder.start()
                MenuIcon.listen(app)
                set_status(app, "Listening…")
                self._emit_event("recording_start", mode="hold", beep=beep_on_start)
                try:
                    show_hold_badge()
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
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

        self._emit_event(
            "hotkey",
            key_type=key_type,
            action=action,
            assistive=is_assistive,
            state=self.state,
        )

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
            await self._handle_hold_event(app, str(action), beep_on_start)
        elif key_type == "toggle" and action == "press":
            await self._handle_toggle_event(app, beep_on_start)

    async def _handle_hold_event(self, app: rumps.App, action: str, beep_on_start: bool) -> None:
        if action == "down" and self.state == "IDLE":
            await self._cancel_pending_hold_start()
            self._hold_start_task = asyncio.create_task(
                self._start_recording_after_delay(app, beep_on_start=beep_on_start)
            )
        elif action == "up":
            if self.state == "REC_HOLD":
                logger.info("Hold released; finishing recording")
                await self.finish_recording(app)
            else:
                await self._cancel_pending_hold_start()

    async def _handle_toggle_event(self, app: rumps.App, beep_on_start: bool) -> None:
        if self.state == "IDLE":
            async with self._serial_lock:
                if self.state != "IDLE":
                    return
                if not client.start_server_if_needed():
                    logger.error("Toggle-start aborted: VistaScribeServer unavailable")
                    self._emit_event("toggle_ignored", reason="backend_unavailable")
                    return
                if not self._session_id:
                    self._session_id = uuid.uuid4().hex
                try:
                    await self.recorder.start()
                    MenuIcon.listen(app)
                    set_status(app, "Listening…")
                    if beep_on_start:
                        start_sound()
                    self._emit_event("recording_start", mode="toggle", beep=beep_on_start)
                    try:
                        show_hold_badge()
                    except Exception as exc:
                        logger.debug("Suppressed exception", exc_info=exc)
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
