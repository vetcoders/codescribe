import json
from pathlib import Path

from vistascribe.config import (
    Config,
    load_config,
    save_config,
    serialize_env,
    update_env_vars,
)
from vistascribe.settings_store import reset_settings_for_tests


def _write_settings(tmp_path, monkeypatch, **data):
    path = tmp_path / "settings.json"
    content = {
        "ai_formatting_enabled": False,
        "ai_provider": "harmony",
        "language": "auto",
    }
    content.update(data)
    path.write_text(json.dumps(content), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
    reset_settings_for_tests()


def test_load_config_truthy_and_defaults(monkeypatch, tmp_path):
    _write_settings(tmp_path, monkeypatch, ai_formatting_enabled=True, language="pl")
    env = {
        "WHISPER_SERVER_URL": "",
        "LLM_SERVER_URL": "http://localhost:9999",
        "WHISPER_LANGUAGE": "PL",
    }
    cfg = load_config(env)
    assert cfg.whisper_url == ""
    assert cfg.llm_url == "http://localhost:9999"
    assert cfg.format_enabled is True
    assert cfg.language == "pl"

    _write_settings(tmp_path, monkeypatch, ai_formatting_enabled=False)
    assert load_config(env).format_enabled is False


def test_serialize_env_ordering_and_roundtrip(monkeypatch, tmp_path):
    _write_settings(tmp_path, monkeypatch, ai_formatting_enabled=True)
    base = {"FOO": "1", "BAR": "2", "LLM_SERVER_URL": "x"}
    cfg = Config(whisper_url="a", llm_url="b", format_enabled=True, language="en")
    s = serialize_env(cfg, base)
    lines = s.strip().split("\n")
    # Core keys first in fixed order
    assert lines[:3] == [
        "WHISPER_SERVER_URL=a",
        "LLM_SERVER_URL=b",
        "WHISPER_LANGUAGE=en",
    ]
    # Others sorted
    assert set(lines[3:]) == {"BAR=2", "FOO=1"}


def test_save_and_update_env_vars(tmp_path: Path, monkeypatch):
    env_path = tmp_path / ".env"
    # create initial file
    env_path.write_text("FOO=1\n", encoding="utf-8")

    _write_settings(tmp_path, monkeypatch, ai_formatting_enabled=False)

    cfg = Config(whisper_url="", llm_url="", format_enabled=False, language=None)
    # save_config defaults to module dir; override with path
    save_config(cfg, path=str(env_path))
    content1 = env_path.read_text(encoding="utf-8")
    assert "FORMAT_ENABLED" not in content1

    update_env_vars({"FOO": "9", "NEWKEY": "XYZ"}, path=str(env_path))
    content2 = env_path.read_text(encoding="utf-8")
    assert "FOO=9" in content2
    assert "NEWKEY=XYZ" in content2
