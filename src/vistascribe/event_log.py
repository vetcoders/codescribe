"""Utility helpers to capture UX events (hotkeys, menu selections, etc.)."""

from __future__ import annotations

import json
import logging
import threading
import time
from typing import Any

from .path_utils import repo_root

_LOG_PATH = repo_root() / "logs" / "events.log"
_LOCK = threading.Lock()
logger = logging.getLogger(__name__)


def _write_entry(entry: dict[str, Any]) -> None:
    try:
        _LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(entry, ensure_ascii=False)
        with _LOCK:
            with _LOG_PATH.open("a", encoding="utf-8") as handle:
                handle.write(line + "\n")
    except Exception as exc:  # pragma: no cover - best effort logging
        logger.debug("Failed to write event log entry: %s", exc)


def log_event(category: str, action: str, **fields: Any) -> None:
    entry = {
        "ts": time.time(),
        "category": category,
        "action": action,
    }
    for key, value in fields.items():
        if value is not None:
            entry[key] = value
    _write_entry(entry)


def log_hotkey_event(kind: str, action: str, **fields: Any) -> None:
    log_event("hotkey", f"{kind}:{action}", **fields)


def log_menu_event(title: str, action: str = "select", **fields: Any) -> None:
    log_event("menu", action, title=title, **fields)


def instrument_menu_item(item, action: str = "select") -> None:
    """Wrap the item's callback so we log whenever it fires."""

    callback = getattr(item, "callback", None)
    if callback is None or getattr(item, "_vs_evt_wrapped", False):
        return

    def _wrapped(sender):  # pragma: no cover - UI callback
        try:
            log_menu_event(item.title or "", action=action)
        except Exception as exc:
            logger.debug("Menu event log failed: %s", exc)
        return callback(sender)

    _wrapped.__name__ = getattr(callback, "__name__", "menu_callback")
    item.set_callback(_wrapped)
    item._vs_evt_wrapped = True
