"""Mix-in providing hold/toggle hotkey menu handlers."""

from __future__ import annotations

import os

import rumps

from ...config import update_env_vars
from ...hotkeys import (
    get_toggle_trigger as hotkeys_get_toggle_trigger,
    hold_mods_label as hotkeys_hold_mods_label,
    is_hold_exclusive as hotkeys_is_hold_exclusive,
    set_hold_exclusive as hotkeys_set_hold_exclusive,
    set_hold_mods as hotkeys_set_hold_mods,
    set_toggle_trigger as hotkeys_set_toggle_trigger,
)
from ..menu_utils import create_parent_item, set_submenu


class HoldMenuMixin:
    def _init_hold_menu(self):
        agent_name = os.environ.get("AGENT_NAME", "El Niño")
        self.item_hold_ctrl = rumps.MenuItem(
            "Hold: Ctrl only (Formatting)", callback=self._set_hold_ctrl
        )
        self.item_hold_ctrl_opt = rumps.MenuItem(
            "Hold: Ctrl+Option", callback=self._set_hold_ctrl_opt
        )
        self.item_hold_ctrl_shift = rumps.MenuItem(
            f"Hold: Ctrl+Shift ({agent_name} AI)", callback=self._set_hold_ctrl_shift
        )
        self.item_hold_ctrl_cmd = rumps.MenuItem(
            "Hold: Ctrl+Command", callback=self._set_hold_ctrl_cmd
        )
        self.item_hold_excl = rumps.MenuItem(
            "Exclusive (ignore extra modifiers)", callback=self._toggle_hold_exclusive
        )
        label = hotkeys_hold_mods_label()
        self.item_hold_current = rumps.MenuItem(f"Current: {label}", callback=lambda _s: None)
        trigger_label = self._toggle_label(hotkeys_get_toggle_trigger())
        self.item_toggle_current = rumps.MenuItem(
            f"Toggle: {trigger_label}", callback=lambda _s: None
        )
        self.item_toggle_opt = rumps.MenuItem(
            "Use double Option (⌥⌥)",
            callback=lambda _s: self._set_toggle_trigger("double_option"),
        )
        self.item_toggle_ralt = rumps.MenuItem(
            "Use double Right Option",
            callback=lambda _s: self._set_toggle_trigger("double_ralt"),
        )
        self.item_toggle_none = rumps.MenuItem(
            "Disable toggle",
            callback=lambda _s: self._set_toggle_trigger("none"),
        )
        self.item_toggle_save = rumps.MenuItem(
            "Save Hotkeys to .env", callback=self._save_hotkeys_env
        )
        set_items = [
            self.item_hold_current,
            None,
            self.item_hold_ctrl,
            self.item_hold_ctrl_opt,
            self.item_hold_ctrl_shift,
            self.item_hold_ctrl_cmd,
            None,
            self.item_hold_excl,
            None,
            self.item_toggle_current,
            self.item_toggle_opt,
            self.item_toggle_ralt,
            self.item_toggle_none,
            None,
            self.item_toggle_save,
        ]
        self.menu_hold = create_parent_item("Hold Hotkeys")
        self.menu_hotkeys = self.menu_hold
        set_submenu(self.menu_hold, set_items)
        self.menu_hotkeys = self.menu_hold
        self._refresh_hold_menu()

    def _rebuild_hold_menu(self) -> None:
        set_items = [
            self.item_hold_current,
            None,
            self.item_hold_ctrl,
            self.item_hold_ctrl_opt,
            self.item_hold_ctrl_shift,
            self.item_hold_ctrl_cmd,
            None,
            self.item_hold_excl,
            None,
            self.item_toggle_current,
            self.item_toggle_opt,
            self.item_toggle_ralt,
            self.item_toggle_none,
            None,
            self.item_toggle_save,
        ]
        set_submenu(self.menu_hold, set_items)

    def _refresh_hold_menu(self) -> None:
        label = hotkeys_hold_mods_label()
        purpose = ""
        if label == "Ctrl":
            purpose = " (Formatting)"
        elif label == "Ctrl+Shift":
            agent_name = os.environ.get("AGENT_NAME", "El Niño")
            purpose = f" ({agent_name} AI)"
        try:
            self.item_hold_current.title = f"Current: {label}{purpose}"
        except Exception:
            pass
        self.item_hold_ctrl.state = label == "Ctrl"
        self.item_hold_ctrl_opt.state = label == "Ctrl+Option"
        self.item_hold_ctrl_shift.state = label == "Ctrl+Shift"
        self.item_hold_ctrl_cmd.state = label == "Ctrl+Command"
        self.item_hold_excl.state = hotkeys_is_hold_exclusive()
        self._refresh_toggle_menu()

    def _set_hold_ctrl(self, _sender):
        self._set_hold_mods("ctrl")

    def _set_hold_ctrl_opt(self, _sender):
        self._set_hold_mods("ctrl+alt")

    def _set_hold_ctrl_shift(self, _sender):
        self._set_hold_mods("ctrl+shift")

    def _set_hold_ctrl_cmd(self, _sender):
        self._set_hold_mods("ctrl+cmd")

    def _set_hold_mods(self, spec: str) -> None:
        os.environ["HOLD_MODS"] = spec
        hotkeys_set_hold_mods(spec)
        try:
            update_env_vars({"HOLD_MODS": spec})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _toggle_hold_exclusive(self, _sender):
        new_flag = not hotkeys_is_hold_exclusive()
        os.environ["HOLD_EXCLUSIVE"] = "1" if new_flag else "0"
        hotkeys_set_hold_exclusive(new_flag)
        try:
            update_env_vars({"HOLD_EXCLUSIVE": "1" if new_flag else "0"})
        except Exception:
            pass
        self._refresh_hold_menu()

    def _set_toggle_trigger(self, trigger: str):
        hotkeys_set_toggle_trigger(trigger)
        os.environ["TOGGLE_TRIGGER"] = trigger
        try:
            update_env_vars({"TOGGLE_TRIGGER": trigger})
        except Exception:
            pass
        self._refresh_toggle_menu()

    def _refresh_toggle_menu(self):
        trigger = os.environ.get("TOGGLE_TRIGGER", "double_option").strip() or "double_option"
        if trigger not in {"double_option", "double_ralt", "none"}:
            trigger = "double_option"
            os.environ["TOGGLE_TRIGGER"] = trigger
        self.item_toggle_current.title = f"Toggle: {self._toggle_label(trigger)}"
        self.item_toggle_opt.state = trigger == "double_option"
        self.item_toggle_ralt.state = trigger == "double_ralt"
        self.item_toggle_none.state = trigger == "none"

    @staticmethod
    def _toggle_label(trigger: str) -> str:
        return {
            "double_option": "double option",
            "double_ralt": "double right option",
            "none": "disabled",
        }.get(trigger, "double option")

    def _save_hotkeys_env(self, _sender):
        mods = os.environ.get("HOLD_MODS", "ctrl")
        excl = os.environ.get("HOLD_EXCLUSIVE", "1" if mods in ("ctrl", "control") else "0")
        try:
            update_env_vars({"HOLD_MODS": mods, "HOLD_EXCLUSIVE": excl})
            rumps.notification(
                title="VistaScribe",
                subtitle="Hotkeys saved",
                message=f"HOLD_MODS={mods}, EXCLUSIVE={excl}",
            )
        except Exception as e:
            import logging

            logging.getLogger(__name__).error("Failed to save hotkeys to .env: %s", e)
