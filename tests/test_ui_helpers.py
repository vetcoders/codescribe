import json
from types import SimpleNamespace

from vistascribe.settings_store import reset_settings_for_tests
from vistascribe.ui import MenuIcon, backend_status_labels, config_labels


def _seed_settings(tmp_path, monkeypatch, **data):
    path = tmp_path / "settings.json"
    base = {
        "ai_formatting_enabled": False,
        "ai_provider": "harmony",
    }
    base.update(data)
    path.write_text(json.dumps(base), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
    reset_settings_for_tests()


def test_backend_status_labels(monkeypatch, tmp_path):
    _seed_settings(tmp_path, monkeypatch, ai_formatting_enabled=False)
    assert backend_status_labels(True, False) == ["STT: OK", "AI: Light+ only"]

    _seed_settings(tmp_path, monkeypatch, ai_formatting_enabled=True, ai_provider="ollama")
    assert backend_status_labels(False, True) == ["STT: OFF", "AI: Ollama (OK)"]


def test_config_labels_simple():
    cfg = SimpleNamespace(
        language=None,
        format_enabled=False,
        whisper_url="",
        llm_url="http://x",
        ai_provider="harmony",
    )
    labels = config_labels(cfg)
    assert labels[0] == "Language: auto"
    assert labels[1] == "AI Formatting: OFF (harmony)"
    assert labels[2] == "Whisper URL: local"
    assert labels[3] == "Harmony URL: http://x"


def test_menuicon_set_behavior_no_appkit(monkeypatch):
    # Ensure glyph hidden when icon present and SHOW_TRAY_GLYPH=0
    monkeypatch.setenv("SHOW_TRAY_GLYPH", "0")
    app = SimpleNamespace(title=None, icon="/tmp/icon.png")
    MenuIcon.set(app, MenuIcon.IDLE)
    assert app.title == ""  # hidden next to image

    # When SHOW_TRAY_GLYPH=1 or no icon, glyph should be set
    monkeypatch.setenv("SHOW_TRAY_GLYPH", "1")
    app2 = SimpleNamespace(title=None, icon=None)
    MenuIcon.set(app2, MenuIcon.THINK)
    assert app2.title == MenuIcon.THINK
