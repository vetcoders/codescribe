import importlib
import sys

import pytest

from tests.helpers.fake_hotkeys import build_fake_hotkeys_module
from tests.helpers.fake_rumps import MenuItem, build_fake_rumps_module

MODULES_WITH_RUMPS = [
    "vistascribe.permission_manager",
    "vistascribe.menu_formatting",
    "vistascribe.app.status",
    "vistascribe.app.controllers.history",
    "vistascribe.app.controllers.models",
    "vistascribe.app.menu_utils",
    "vistascribe.app.mixins.appearance",
    "vistascribe.app.mixins.backends",
    "vistascribe.app.mixins.feedback",
    "vistascribe.app.mixins.hold_menu",
    "vistascribe.app.mixins.runtime_loop",
    "vistascribe.app.mixins.tools",
    "vistascribe.app.recording_controller",
]


class DummyRecorder:
    def __init__(self):
        self.last_duration = 0.0

    async def stop(self):  # pragma: no cover - tests never await
        return None


@pytest.fixture
def tray_runtime(monkeypatch):
    fake_rumps = build_fake_rumps_module()
    fake_hotkeys = build_fake_hotkeys_module()
    monkeypatch.setitem(sys.modules, "rumps", fake_rumps)
    monkeypatch.setitem(sys.modules, "vistascribe.hotkeys", fake_hotkeys)

    import vistascribe.audio as audio

    monkeypatch.setattr(audio, "Recorder", DummyRecorder)

    import vistascribe.first_run as first_run

    monkeypatch.setattr(first_run, "ensure_config_and_permissions", lambda: None)

    import vistascribe.diag as diag

    monkeypatch.setattr(diag, "run_preflight", lambda _logger: {})
    monkeypatch.setattr(diag, "write_snapshot", lambda _info, _root: None)

    saved = {}
    for name in MODULES_WITH_RUMPS:
        saved[name] = sys.modules.pop(name, None)
    sys.modules.pop("vistascribe.app.runtime", None)

    runtime = importlib.import_module("vistascribe.app.runtime")

    yield runtime, fake_rumps

    sys.modules.pop("vistascribe.app.runtime", None)
    for name in MODULES_WITH_RUMPS:
        sys.modules.pop(name, None)
        if saved[name] is not None:
            sys.modules[name] = saved[name]
    sys.modules.pop("rumps", None)
    sys.modules.pop("vistascribe.hotkeys", None)


def _visible_titles(menu, fake_rumps_module):
    titles = []
    for entry in menu.ordered():
        if isinstance(entry, fake_rumps_module.MenuItem):
            titles.append(entry.title)
    return titles


def test_tray_menu_order_and_callbacks(tray_runtime):
    runtime, fake_rumps = tray_runtime
    app = runtime.VistaScribe()

    titles = _visible_titles(app.menu, fake_rumps)
    expected = [
        "Status: Initializing...",
        "Enable Hotkeys",
        "Language",
        "Formatting",
        "Hold Hotkeys",
        "Models",
        "Backends",
        "History",
        "Appearance",
        "Feedback",
        "Permissions",
        "Tools",
        "What do these toggles do?",
        "Start at Login",
        "Quit...",
    ]
    assert titles == expected

    for label in ("Enable Hotkeys", "What do these toggles do?", "Start at Login", "Quit..."):
        item = app.menu[label]
        assert isinstance(item, MenuItem)
        assert callable(item.callback)

    for submenu in (
        app.menu_models,
        app.menu_backends,
        app.menu_history,
        app.menu_permissions,
        app.menu_tools,
    ):
        assert submenu.menu is not None
        assert any(
            isinstance(child, MenuItem) for child in submenu.menu.ordered() if child is not None
        )
