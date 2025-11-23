"""
diag.py — developer diagnostics for VistaScribe

Lightweight, safe preflight checks that can be enabled via DEV_MODE=1 or
the quickstart flags --dev/--verbose. Results are logged via the provided
logger, without raising exceptions or blocking the app.
"""

from __future__ import annotations

import json
import logging
import os
import platform
import sys
from datetime import datetime
from pathlib import Path

logger = logging.getLogger(__name__)


def _safe_imports():
    AppKit = Quartz = sd = None
    try:
        import AppKit as _AppKit  # type: ignore

        AppKit = _AppKit
    except Exception:
        AppKit = None
    try:
        import Quartz as _Quartz  # type: ignore

        Quartz = _Quartz
    except Exception:
        Quartz = None
    try:
        import sounddevice as _sd  # type: ignore

        sd = _sd
    except Exception:
        sd = None
    return AppKit, Quartz, sd


def _bool(b) -> str:
    return "YES" if b else "NO"


def run_preflight(logger) -> dict:
    """Collect a snapshot of environment and capability checks.

    Returns a dict for optional JSON dumping.
    """
    AppKit, Quartz, sd = _safe_imports()

    info: dict[str, object] = {
        "time": datetime.now().isoformat(timespec="seconds"),
        "python": sys.version.split(" ")[0],
        "executable": sys.executable,
        "platform": platform.platform(),
        "arch": platform.machine(),
        "cwd": os.getcwd(),
        "venv": os.environ.get("VIRTUAL_ENV", ""),
        "dev_mode": os.environ.get("DEV_MODE", "0"),
        "log_level": os.environ.get("LOG_LEVEL", ""),
    }

    logger.info("[DIAG] Python: %s (%s) exec=%s", info["python"], info["arch"], info["executable"])
    logger.info("[DIAG] macOS: %s", info["platform"])
    logger.info("[DIAG] CWD=%s VENV=%s", info["cwd"], info["venv"])

    # Accessibility (AX) trust check
    ax_trusted = None
    try:
        if Quartz is not None:
            try:
                opts = {Quartz.kAXTrustedCheckOptionPrompt: False}
                ax_trusted = Quartz.AXIsProcessTrustedWithOptions(opts)
            except Exception:
                # Fallback for older PyObjC/macOS
                ax_trusted = Quartz.AXIsProcessTrusted()
            logger.info("[DIAG] Accessibility trusted: %s", _bool(bool(ax_trusted)))
        else:
            logger.info("[DIAG] Accessibility frameworks not available (Quartz import failed)")
    except Exception as e:
        logger.warning("[DIAG] Accessibility check failed: %s", e)

    # Event tap quick probe (indicates Input Monitoring readiness)
    tap_ok = None
    try:
        if Quartz is not None:
            mask = Quartz.CGEventMaskBit(Quartz.kCGEventKeyDown) | Quartz.CGEventMaskBit(
                Quartz.kCGEventKeyUp
            )
            tap = Quartz.CGEventTapCreate(
                Quartz.kCGSessionEventTap,
                Quartz.kCGHeadInsertEventTap,
                0,
                mask,
                lambda *_args: None,
                None,
            )
            tap_ok = bool(tap)
            logger.info(
                "[DIAG] Event tap creatable: %s (Input Monitoring likely %s)",
                _bool(tap_ok),
                _bool(tap_ok),
            )
            # Clean up if created
            if tap:
                Quartz.CFMachPortInvalidate(tap)
        else:
            logger.info("[DIAG] Event tap check skipped (Quartz unavailable)")
    except Exception as e:
        tap_ok = False
        logger.warning("[DIAG] Event tap probe failed: %s", e)

    # Microphone basic probe
    default_input = {}
    try:
        if sd is not None:
            di = sd.query_devices(kind="input")
            default_input = {k: di.get(k) for k in ("name", "index", "max_input_channels")}
            logger.info("[DIAG] Mic default: %s", default_input)
        else:
            logger.info("[DIAG] sounddevice not available; mic probe skipped")
    except Exception as e:
        logger.warning("[DIAG] Mic probe failed: %s", e)

    # Status bar presence (best-effort)
    try:
        if AppKit is not None:
            nsapp = AppKit.NSApp()
            # nsapp may be None before run loop; log what we can
            logger.info("[DIAG] NSApp present: %s", _bool(bool(nsapp)))
        else:
            logger.info("[DIAG] AppKit not available; statusbar probe skipped")
    except Exception as e:
        logger.warning("[DIAG] Status bar probe failed: %s", e)

    info.update(
        {
            "accessibility_trusted": bool(ax_trusted) if ax_trusted is not None else None,
            "event_tap_creatable": tap_ok,
            "mic_default": default_input,
        }
    )
    return info


def write_snapshot(info: dict, repo_root: str | None = None) -> None:
    try:
        root = Path(repo_root or os.getcwd())
        logs = root / "logs"
        logs.mkdir(parents=True, exist_ok=True)
        path = logs / f"diagnostics-{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        path.write_text(json.dumps(info, indent=2, ensure_ascii=False), encoding="utf-8")
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)
