"""Minimal hotkeys stub for unit tests (no Quartz dependency)."""

from __future__ import annotations

import queue
import types


def _label_for_spec(spec: str) -> str:
    parts = []
    normalized = (spec or "").lower().split("+")
    for part in normalized:
        part = part.strip()
        if part in {"ctrl", "control"}:
            parts.append("Ctrl")
        elif part in {"alt", "option", "opt"}:
            parts.append("Option")
        elif part == "shift":
            parts.append("Shift")
        elif part in {"cmd", "command", "meta"}:
            parts.append("Command")
    return "+".join(parts) or "Ctrl"


def build_fake_hotkeys_module():
    state = {
        "mods": "ctrl",
        "exclusive": True,
        "toggle": "double_option",
        "queue": queue.Queue(),
    }

    module = types.ModuleType("vistascribe.hotkeys")

    def events():
        return state["queue"]

    def start():
        return True

    def stop():  # pragma: no cover - trivial
        return True

    def set_hold_mods(spec: str):
        state["mods"] = (spec or "ctrl").lower()

    def hold_mods_label() -> str:
        return _label_for_spec(state["mods"])

    def set_hold_exclusive(flag: bool):
        state["exclusive"] = bool(flag)

    def is_hold_exclusive() -> bool:
        return state["exclusive"]

    def set_toggle_trigger(trigger: str):
        state["toggle"] = (trigger or "double_option").lower()

    def get_toggle_trigger() -> str:
        return state["toggle"]

    module.events = events
    module.start = start
    module.stop = stop
    module.set_hold_mods = set_hold_mods
    module.hold_mods_label = hold_mods_label
    module.set_hold_exclusive = set_hold_exclusive
    module.is_hold_exclusive = is_hold_exclusive
    module.set_toggle_trigger = set_toggle_trigger
    module.get_toggle_trigger = get_toggle_trigger

    # Additional helpers used in runtime/mixins
    module.set_hold_mods_label = set_hold_mods  # alias for legacy imports

    return module


__all__ = ["build_fake_hotkeys_module"]
