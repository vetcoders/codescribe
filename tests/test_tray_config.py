import importlib
import json

from vistascribe.settings_store import reset_settings_for_tests


def _seed_settings(tmp_path, monkeypatch, **overrides):
    path = tmp_path / "settings.json"
    content = {
        "language": "auto",
        "ai_formatting_enabled": False,
        "ai_provider": "harmony",
        "ai_max_tokens": 512,
        "ai_assistive_max_tokens": 2048,
    }
    content.update(overrides)
    path.write_text(json.dumps(content), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
    reset_settings_for_tests()


def test_load_config_from_env(monkeypatch, tmp_path):
    # Prepare environment
    monkeypatch.setenv("WHISPER_SERVER_URL", "http://127.0.0.1:8238")
    monkeypatch.setenv("LLM_SERVER_URL", "http://127.0.0.1:8239")
    monkeypatch.setenv("WHISPER_LANGUAGE", "pl")
    _seed_settings(tmp_path, monkeypatch, ai_formatting_enabled=True, language="pl")

    import vistascribe.config as cfg

    importlib.reload(cfg)

    c = cfg.load_config()
    assert c.whisper_url == "http://127.0.0.1:8238"
    assert c.llm_url == "http://127.0.0.1:8239"
    assert c.format_enabled is True
    assert c.language == "pl"


def test_serialize_env_and_save(tmp_path, monkeypatch):
    import vistascribe.config as cfg

    _seed_settings(tmp_path, monkeypatch, ai_formatting_enabled=False)

    c = cfg.Config(
        whisper_url="",
        llm_url="http://x",
        format_enabled=False,
        language=None,
    )

    content = cfg.serialize_env(c)
    assert "LLM_SERVER_URL=http://x" in content
    assert "WHISPER_SERVER_URL=" in content  # empty means local
    assert "WHISPER_LANGUAGE=" in content
    assert "FORMAT_ENABLED" not in content

    env_path = tmp_path / ".env"
    cfg.save_config(c, path=str(env_path))
    text = env_path.read_text()
    assert text == content


def test_ui_config_labels(monkeypatch, tmp_path):
    import vistascribe.config as cfg
    import vistascribe.ui as ui_mod
    # ensure we can import without macOS frameworks by not touching UI classes

    _seed_settings(tmp_path, monkeypatch, ai_formatting_enabled=True)

    c = cfg.Config(
        whisper_url="",
        llm_url="",
        format_enabled=True,
        language=None,
        ai_provider="harmony",
    )
    labels = ui_mod.config_labels(c)
    assert "Language: auto" in labels[0]
    assert "AI Formatting: ON" in labels[1]
    assert "Whisper URL: local" in labels[2]
    assert "Harmony URL: local" in labels[3]

    c2 = cfg.Config(
        whisper_url="http://a",
        llm_url="http://b",
        format_enabled=False,
        language="en",
        ai_provider="ollama",
    )
    labels2 = ui_mod.config_labels(c2)
    assert labels2[0].endswith("en")
    assert labels2[1].endswith("OFF (ollama)")
    assert labels2[2].endswith("http://a")
    assert labels2[3].endswith("http://b")
