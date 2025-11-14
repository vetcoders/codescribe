"""Permission utilities shared by the tray runtime."""

from __future__ import annotations

import logging
import subprocess
import threading
from typing import Any

import rumps

from . import diag

logger = logging.getLogger(__name__)


class PermissionManager:
    """Mixin that encapsulates macOS permission probes + menu helpers."""

    _perm_snapshot: dict[str, Any] | None = None

    # ---------------------------------------------------------------------
    # Formatting helpers
    # ---------------------------------------------------------------------
    def _permission_symbol(self, flag: Any) -> str:
        if flag is True:
            return "✓"
        if flag is False:
            return "!"
        return "?"

    def _format_permission_summary(self, info: dict[str, Any]) -> tuple[str, str]:
        ax = info.get("accessibility_trusted")
        tap_ok = info.get("event_tap_creatable")
        mic_info = info.get("mic_default") or {}
        mic_ok = bool(mic_info.get("name"))
        status_line = (
            f"Status: AX {self._permission_symbol(ax)} • "
            f"Hotkeys {self._permission_symbol(tap_ok)} • "
            f"Mic {self._permission_symbol(mic_ok)}"
        )
        details: list[str] = []
        details.append(
            "Accessibility: granted"
            if ax
            else "Accessibility: missing (enable VistaScribe in Accessibility)."
        )
        details.append(
            "Hotkeys/Input Monitoring: ready"
            if tap_ok
            else ("Hotkeys/Input Monitoring: missing (enable VistaScribe in Input Monitoring).")
        )
        details.append(
            f"Microphone: {mic_info.get('name', 'available')}"
            if mic_ok
            else "Microphone: permission not granted."
        )
        return status_line, "\n".join(details)

    def _update_permissions_menu(self, snapshot: dict[str, Any] | None = None) -> None:
        info = snapshot or self._perm_snapshot or {}
        status_line, _ = self._format_permission_summary(info)
        if hasattr(self, "item_perm_status"):
            self.item_perm_status.title = status_line

    # ---------------------------------------------------------------------
    # Probing & scheduling
    # ---------------------------------------------------------------------
    def _run_permission_probe(self, show_alert: bool = False) -> None:
        try:
            info = diag.run_preflight(logger)
            self._perm_snapshot = info

            def _update() -> None:
                self._update_permissions_menu(info)
                if show_alert:
                    _, details = self._format_permission_summary(info)
                    rumps.alert(title="Permission Check", message=details, ok="OK")

            helper = getattr(rumps, "AppHelper", None)
            if helper is not None:
                helper.call_after(_update)
            else:
                _update()
        except Exception as exc:  # pragma: no cover - defensive UI path
            logger.error("Permission probe failed: %s", exc)
            if show_alert:
                rumps.alert(
                    title="Permission Check",
                    message=(
                        "Could not evaluate permissions automatically. "
                        "Please verify System Settings manually."
                    ),
                    ok="OK",
                )

    def _schedule_permission_probe(self) -> None:
        threading.Thread(
            target=self._run_permission_probe,
            kwargs={"show_alert": False},
            daemon=True,
        ).start()

    # ---------------------------------------------------------------------
    # Menu callbacks
    # ---------------------------------------------------------------------
    def _check_permissions(self, _sender) -> None:
        self._run_permission_probe(show_alert=True)

    def _prompt_accessibility_permission(self, _sender) -> None:
        try:
            import Quartz

            Quartz.AXIsProcessTrustedWithOptions({Quartz.kAXTrustedCheckOptionPrompt: True})
        except Exception as exc:  # pragma: no cover - depends on macOS TCC state
            logger.error("Accessibility prompt failed: %s", exc)
            rumps.alert(
                title="Accessibility",
                message=(
                    "Open System Settings → Privacy & Security → Accessibility "
                    "and enable VistaScribe."
                ),
                ok="OK",
            )
        self._schedule_permission_probe()

    def _request_microphone_access(self, _sender) -> None:
        granted = False
        try:
            import sounddevice as sd

            sd.rec(1, samplerate=16000, channels=1, dtype="float32")
            sd.wait()
            granted = True
        except Exception as exc:  # pragma: no cover - depends on device setup
            logger.warning("Microphone request failed or denied: %s", exc)
        if granted:
            try:
                rumps.notification(
                    title="VistaScribe",
                    subtitle="Microphone",
                    message="Permission granted.",
                )
            except Exception:  # pragma: no cover
                pass
        else:
            rumps.alert(
                title="Microphone",
                message=(
                    "Permission not granted. Open System Settings → Privacy & Security → "
                    "Microphone and enable VistaScribe."
                ),
                ok="OK",
            )
        self._schedule_permission_probe()

    # ---------------------------------------------------------------------
    # System Settings helpers
    # ---------------------------------------------------------------------
    def _open_privacy_anchor(self, anchor: str, fallback: str) -> None:
        try:
            script = (
                'tell application "System Settings"\n'
                "  activate\n"
                f'  reveal anchor "{anchor}" of pane id '
                '"com.apple.settings.PrivacySecurity.extension"\n'
                "end tell"
            )
            subprocess.run(["osascript", "-e", script], check=False)
        except Exception as exc:  # pragma: no cover
            logger.error("Failed to open System Settings anchor %s: %s", anchor, exc)
            rumps.alert(title="System Settings", message=fallback, ok="OK")

    def _open_input_monitoring_settings(self, _sender) -> None:
        self._open_privacy_anchor(
            "Privacy_ListenEvent",
            (
                "Open System Settings → Privacy & Security → Input Monitoring and "
                "enable VistaScribe."
            ),
        )

    def _open_accessibility(self, _sender) -> None:
        self._open_privacy_anchor(
            "Privacy_Accessibility",
            ("Open System Settings → Privacy & Security → Accessibility and enable VistaScribe."),
        )

    def _open_microphone_settings(self, _sender) -> None:
        self._open_privacy_anchor(
            "Privacy_Microphone",
            ("Open System Settings → Privacy & Security → Microphone and enable VistaScribe."),
        )
