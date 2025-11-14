"""Shared helpers for building rumps submenus."""

from __future__ import annotations

import logging
from collections.abc import Callable, Iterable

import rumps

from ..event_log import instrument_menu_item

logger = logging.getLogger(__name__)


def _parent_noop(_sender: rumps.MenuItem) -> None:
    return None


def create_parent_item(
    title: str, callback: Callable[[rumps.MenuItem], None] | None = None
) -> rumps.MenuItem:
    """Return a MenuItem that acts as a submenu parent but stays enabled."""

    return rumps.MenuItem(title, callback or _parent_noop)


def ensure_parent_callback(menu_item: rumps.MenuItem) -> None:
    """Guarantee that a parent MenuItem always has a callable callback."""

    if menu_item.callback is None:
        menu_item.set_callback(_parent_noop)


def set_submenu(menu_item: rumps.MenuItem, entries: Iterable[rumps.MenuItem | None]):
    """Replace (or attach) the children of a submenu with new entries."""

    raw_entries = list(entries)
    ensure_parent_callback(menu_item)

    def _apply() -> None:
        processed: list[rumps.MenuItem | None] = []
        for entry in raw_entries:
            if entry is None:
                processed.append(None)
            else:
                instrument_menu_item(entry)
                processed.append(entry)

        if getattr(menu_item, "_menu", None) is None:
            try:
                menu_item.add(None)  # create backing NSMenu
            except Exception:
                logger.exception("Failed to initialize submenu for %s", menu_item.title)
                return

        try:
            menu_item.clear()
        except Exception:
            logger.exception("Failed to clear submenu for %s", menu_item.title)
            return

        public_entries: list[rumps.MenuItem] = []
        for entry in processed:
            if entry is None:
                menu_item.add(None)
            else:
                menu_item.add(entry)
                public_entries.append(entry)

        instrument_menu_item(menu_item)
        try:
            menu_item.menu = list(public_entries)
        except Exception:
            logger.debug("Could not expose menu iterable on %s", menu_item.title)
        try:
            count = len(menu_item)
        except Exception:
            count = -1
        logger.info("Submenu %s items=%s", menu_item.title, count)
        if getattr(menu_item, "_menu", None) is None or count == 0:
            logger.error("Submenu %s still missing after rebuild", menu_item.title)

    helper = getattr(rumps, "AppHelper", None)
    if helper is not None and getattr(menu_item, "_menu", None) is not None:
        helper.call_after(_apply)
    else:
        _apply()
