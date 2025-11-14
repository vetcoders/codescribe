import sys
import types

import vistascribe.menu_model as mm


class FakeMenuItem:
    def __init__(self, title, callback=None):
        self.title = title
        self.callback = callback

    def set_callback(self, cb):
        self.callback = cb


class FakeRumpsModule(types.ModuleType):
    def __init__(self):
        super().__init__("rumps")
        self.MenuItem = FakeMenuItem
        self.alert_calls = []

    def alert(self, title="", message=""):
        self.alert_calls.append((title, message))


def test_render_rumps_menu_with_stub(monkeypatch, caplog):
    fake_rumps = FakeRumpsModule()
    sys.modules["rumps"] = fake_rumps

    called = {"ok": 0}

    def ok_action():
        called["ok"] += 1

    def boom_action():
        raise RuntimeError("boom")

    spec = [
        mm.MenuSpecItem(kind="label", title="Header"),
        mm.MenuSpecItem(kind="action", title="Do OK", action="ok"),
        mm.MenuSpecItem(kind="action", title="Do Boom", action="boom"),
    ]
    rendered = mm.render_rumps_menu(
        app=None,
        spec=spec,
        actions={"ok": ok_action, "boom": boom_action},
        alert_title="TestApp",
    )

    # rendered: [MenuItem, MenuItem, MenuItem]
    assert rendered[0].title == "Header"

    # Trigger OK
    rendered[1].callback(None)
    assert called["ok"] == 1

    # Trigger Boom (should be caught, logged, and not raise)
    with caplog.at_level("ERROR"):
        rendered[2].callback(None)
    assert "Menu action failed: boom" in caplog.text
    assert not fake_rumps.alert_calls

    # Now render without providing 'boom' action -> should be disabled but callable
    rendered2 = mm.render_rumps_menu(
        app=None,
        spec=spec,
        actions={"ok": ok_action},
        alert_title="TestApp",
    )
    assert callable(rendered2[2].callback)
    # Calling missing action shouldn't change state or alert
    prev_alerts = len(fake_rumps.alert_calls)
    rendered2[2].callback(None)
    assert len(fake_rumps.alert_calls) == prev_alerts
