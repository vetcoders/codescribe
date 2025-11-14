"""Lightweight stand-in for the `rumps` module used in unit tests.

The goal is to let menu-building code run without macOS-specific dependencies.
"""

from __future__ import annotations

import types
from collections.abc import Iterable


class MenuList(dict):
    """Ordered container that mimics rumps.Menu behaviour enough for tests."""

    def __init__(self, entries: Iterable[object] | None = None) -> None:
        super().__init__()
        self._ordered: list[object] = []
        if entries:
            for entry in entries:
                self.add(entry)

    def add(self, entry: object | None) -> object | None:
        if entry is None:
            self._ordered.append(None)
            return None
        if isinstance(entry, str):
            entry = MenuItem(entry)
        self._ordered.append(entry)
        if isinstance(entry, MenuItem) and entry.title:
            self[entry.title] = entry
        return entry

    def clear(self) -> None:  # pragma: no cover - trivial
        super().clear()
        self._ordered.clear()

    def ordered(self) -> list[object]:
        return list(self._ordered)

    def __iter__(self):  # pragma: no cover - convenience
        return iter(self._ordered)


class MenuItem:
    def __init__(self, title: str, callback=None):
        self.title = title
        self.callback = callback
        self.state = 0
        self._menu: MenuList | None = None

    def set_callback(self, callback) -> None:
        self.callback = callback

    def add(self, entry: object | None) -> None:
        if self._menu is None:
            self._menu = MenuList()
        self._menu.add(entry)

    def clear(self) -> None:
        if self._menu is not None:
            self._menu.clear()

    @property
    def menu(self) -> MenuList | None:
        return self._menu

    @menu.setter
    def menu(self, entries: Iterable[object | None]) -> None:
        self._menu = MenuList(entries)

    def __getitem__(self, key: str) -> MenuItem:  # pragma: no cover - convenience
        if self._menu is None:
            raise KeyError(key)
        return self._menu[key]

    def keys(self):  # pragma: no cover - convenience
        if self._menu is None:
            return []
        return list(self._menu.keys())


class App:
    def __init__(self, title: str, *, quit_button=None):
        self.title = title
        self.quit_button = quit_button
        self._menu = MenuList()

    @property
    def menu(self) -> MenuList:
        return self._menu

    @menu.setter
    def menu(self, entries):
        self._menu = MenuList(entries)


class Timer:
    def __init__(self, callback, interval):
        self.callback = callback
        self.interval = interval
        self._running = False

    def start(self):  # pragma: no cover - trivial
        self._running = True

    def stop(self):  # pragma: no cover - trivial
        self._running = False


_last_alert = None
_last_notification = None
_quit_called = False


def alert(title="", message="", ok=None):  # pragma: no cover - trivial
    global _last_alert
    _last_alert = (title, message, ok)


def notification(title="", subtitle="", message=""):  # pragma: no cover - trivial
    global _last_notification
    _last_notification = (title, subtitle, message)


def quit_application():  # pragma: no cover - trivial
    global _quit_called
    _quit_called = True


def build_fake_rumps_module():
    module = types.ModuleType("rumps")
    module.MenuItem = MenuItem
    module.Menu = MenuList
    module.Timer = Timer
    module.App = App
    module.alert = alert
    module.notification = notification
    module.quit_application = quit_application
    return module


__all__ = [
    "App",
    "MenuItem",
    "MenuList",
    "Timer",
    "build_fake_rumps_module",
]
