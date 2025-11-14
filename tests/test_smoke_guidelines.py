"""
Simple smoke tests to demonstrate how to run pytest in this repository.
These tests are intentionally dependency-light and should pass without
requiring optional macOS-specific frameworks.
"""


def test_math_sanity():
    # A trivial test that should always pass
    assert 2 + 2 == 4


import json

from vistascribe.settings_store import reset_settings_for_tests


def test_config_roundtrip_minimal(monkeypatch, tmp_path):
    # Demonstrates importing a light, pure-Python module from this repo
    from vistascribe.config import Config, load_config, serialize_env  # local imports

    settings_path = tmp_path / "settings.json"
    settings_path.write_text(json.dumps({"ai_formatting_enabled": True}), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(settings_path))
    reset_settings_for_tests()

    cfg = load_config(
        {
            "WHISPER_SERVER_URL": "",
            "LLM_SERVER_URL": "",
            "WHISPER_LANGUAGE": "en",
        }
    )
    # validates typing and normalization
    assert isinstance(cfg, Config)
    assert cfg.format_enabled is True

    # roundtrip to .env-like text
    s = serialize_env(cfg)
    assert "WHISPER_SERVER_URL=" in s
    assert "LLM_SERVER_URL=" in s
    assert "WHISPER_LANGUAGE=en" in s
