import importlib
import json

from vistascribe.settings_store import reset_settings_for_tests


def test_load_config_handles_non_string_urls_and_defaults(monkeypatch, tmp_path):
    import vistascribe.config as cfg

    settings_path = tmp_path / "settings.json"
    settings_path.write_text(json.dumps({"ai_formatting_enabled": False}), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(settings_path))
    reset_settings_for_tests()

    importlib.reload(cfg)

    env = {
        "WHISPER_SERVER_URL": None,
        "LLM_SERVER_URL": 123,
        "WHISPER_LANGUAGE": 456,
    }
    c = cfg.load_config(env)
    assert isinstance(c.whisper_url, str)
    assert c.whisper_url == ""
    assert isinstance(c.llm_url, str)
    assert c.llm_url == ""
    assert c.format_enabled is False
    assert c.language is None

    text = cfg.serialize_env(c)
    assert "WHISPER_SERVER_URL=" in text
    assert "LLM_SERVER_URL=" in text
    assert "FORMAT_ENABLED" not in text
