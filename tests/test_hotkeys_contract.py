import importlib
import sys

from tests.helpers.fake_hotkeys import build_fake_hotkeys_module
from tests.helpers.fake_rumps import build_fake_rumps_module


def test_hold_mods_label_and_exclusive_toggle(monkeypatch):
    fake_hotkeys = build_fake_hotkeys_module()
    monkeypatch.setitem(sys.modules, "vistascribe.hotkeys", fake_hotkeys)

    hotkeys = importlib.import_module("vistascribe.hotkeys")
    assert hotkeys.hold_mods_label() == "Ctrl"

    hotkeys.set_hold_mods("ctrl+alt")
    assert hotkeys.hold_mods_label() == "Ctrl+Option"

    assert hotkeys.is_hold_exclusive() is True
    hotkeys.set_hold_exclusive(False)
    assert hotkeys.is_hold_exclusive() is False


def test_hold_menu_roundtrip_updates_hotkeys(monkeypatch):
    fake_hotkeys = build_fake_hotkeys_module()
    fake_rumps = build_fake_rumps_module()
    monkeypatch.setitem(sys.modules, "vistascribe.hotkeys", fake_hotkeys)
    monkeypatch.setitem(sys.modules, "rumps", fake_rumps)

    module_name = "vistascribe.app.mixins.hold_menu"
    sys.modules.pop(module_name, None)
    hold_menu = importlib.import_module(module_name)
    saved_env = []
    monkeypatch.setattr(hold_menu, "update_env_vars", lambda values: saved_env.append(values))

    class Dummy(hold_menu.HoldMenuMixin):
        def __init__(self):
            self._init_hold_menu()

    dummy = Dummy()

    dummy._set_hold_ctrl_opt(None)
    assert hold_menu.hotkeys_hold_mods_label() == "Ctrl+Option"
    assert "Ctrl+Option" in dummy.item_hold_current.title

    dummy._toggle_hold_exclusive(None)
    assert hold_menu.hotkeys_is_hold_exclusive() is False

    dummy._set_toggle_trigger("double_ralt")
    assert dummy.item_toggle_current.title.endswith("double right option")

    dummy._save_hotkeys_env(None)
    assert saved_env[-1]["HOLD_MODS"] == "ctrl+alt"

    sys.modules.pop(module_name, None)
