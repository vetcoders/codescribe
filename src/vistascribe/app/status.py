"""Helpers for updating tray status labels."""

from __future__ import annotations

import logging

import rumps

logger = logging.getLogger(__name__)


def set_status(app: rumps.App, text: str) -> None:
    """Update the top "Status" label in the tray menu."""

    try:
        for key in list(app.menu.keys()):
            try:
                item = app.menu[key]
                if isinstance(item, rumps.MenuItem) and str(key).startswith("Status:"):
                    item.title = f"Status: {text}"
                    return
            except Exception:  # pragma: no cover - best effort UI update
                pass
        # Fallback: prepend a new status item if none existed yet
        app.menu.insert(0, rumps.MenuItem(f"Status: {text}"))
    except Exception:  # pragma: no cover
        logger.debug("Failed to update status label", exc_info=True)
