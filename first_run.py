from __future__ import annotations

import json
import os
from pathlib import Path

try:
    import AppKit  # type: ignore
except Exception:  # Allow headless/tests
    AppKit = None  # type: ignore


APP_SUPPORT_DIR = Path.home() / "Library" / "Application Support" / "VistaScribe"
CONFIG_JSON = APP_SUPPORT_DIR / "config.json"


def _ensure_dirs() -> None:
    APP_SUPPORT_DIR.mkdir(parents=True, exist_ok=True)


def load_json() -> dict:
    try:
        return json.loads(CONFIG_JSON.read_text(encoding="utf-8"))
    except Exception:
        return {}


def save_json(cfg: dict) -> None:
    _ensure_dirs()
    CONFIG_JSON.write_text(json.dumps(cfg, indent=2, ensure_ascii=False), encoding="utf-8")


def _nsalert_first_run() -> str | None:
    """Simple first-run chooser for Whisper variant. Returns variant or None."""
    if AppKit is None:
        return None
    alert = AppKit.NSAlert.new()
    alert.setMessageText_("VistaScribe — First time setup")
    alert.setInformativeText_(
        "Choose Whisper model variant. You can change this later in the menu."
    )
    # Buttons are added in reverse order (rightmost first)
    alert.addButtonWithTitle_("Large v3 Turbo")  # index 1000
    alert.addButtonWithTitle_("Large v3")
    alert.addButtonWithTitle_("Medium")
    alert.addButtonWithTitle_("Skip")  # index 1003 (leftmost)
    resp = alert.runModal()
    # Map common modal codes to our options
    # First button is 1000
    if resp == 1000:
        return "large-v3-turbo"
    if resp == 1001:
        return "large-v3"
    if resp == 1002:
        return "medium"
    return None


def ensure_config_and_permissions() -> None:
    """One-time minimal setup after drag-and-drop install.

    - Creates ~/Library/Application Support/VistaScribe/config.json (if missing).
    - Applies sensible defaults: restore clipboard, hold mods, etc.
    - Optionally lets user pick Whisper variant.
    - Nudges user to grant permissions later via app menu (no blocking here).
    """
    cfg = load_json()
    if not cfg:
        variant = _nsalert_first_run()
        cfg = {
            "restore_clipboard": True,
            "hold_mods": "ctrl",
            "hold_exclusive": True,
            "beep_on_start": False,
            "whisper_variant": variant or "large-v3-turbo",
        }
        save_json(cfg)

    # Apply to environment so the current process picks them up
    if cfg.get("restore_clipboard", True):
        os.environ["RESTORE_CLIPBOARD"] = "1"
    os.environ["HOLD_MODS"] = cfg.get("hold_mods", "ctrl+alt")
    os.environ["HOLD_EXCLUSIVE"] = "1" if cfg.get("hold_exclusive", True) else "0"
    if cfg.get("beep_on_start", False):
        os.environ["BEEP_ON_START"] = "1"
    if cfg.get("whisper_variant"):
        os.environ["WHISPER_VARIANT"] = str(cfg.get("whisper_variant"))

    # Persist a subset to .env so next runs are consistent (best-effort)
    try:
        from config import update_env_vars

        updates = {
            "RESTORE_CLIPBOARD": "1" if cfg.get("restore_clipboard", True) else "0",
            "HOLD_MODS": os.environ.get("HOLD_MODS", "ctrl+alt"),
            "HOLD_EXCLUSIVE": os.environ.get("HOLD_EXCLUSIVE", "1"),
            "BEEP_ON_START": os.environ.get("BEEP_ON_START", "0"),
            "WHISPER_VARIANT": os.environ.get("WHISPER_VARIANT", "large-v3-turbo"),
        }
        update_env_vars(updates)
    except Exception:
        pass

    # Best-effort: nudge user to grant Accessibility/Input permissions later via menu
    # We avoid opening System Settings automatically here to keep first run smooth.
