import json

import pytest

import vistascribe.llm as llm
import vistascribe.stt as stt
from vistascribe.settings_store import reset_settings_for_tests


def _settings(tmp_path, monkeypatch, **overrides):
    path = tmp_path / "settings.json"
    data = {
        "ai_formatting_enabled": False,
        "ai_provider": "harmony",
        "ai_max_tokens": 512,
        "ai_assistive_max_tokens": 2048,
    }
    data.update(overrides)
    path.write_text(json.dumps(data), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
    reset_settings_for_tests()


@pytest.mark.parametrize("code, expected", [(None, None), ("pl", "pl"), ("PL", "pl"), ("xx", None)])
def test_stt_language_set_get(monkeypatch, code, expected):
    stt.set_language(code)
    assert stt.get_language() == expected


def test_stt_current_variant_parsing(monkeypatch, tmp_path):
    # Force local mode
    monkeypatch.setattr(stt, "WHISPER_SERVER_URL", "")
    monkeypatch.setattr(stt, "WHISPER_DIR", "/tmp/models/whisper-medium")
    assert stt.get_current_variant() == "medium"
    monkeypatch.setattr(stt, "WHISPER_DIR", "/tmp/models/whisper-large-v3-turbo")
    assert stt.get_current_variant() == "large-v3-turbo"
    monkeypatch.setattr(stt, "WHISPER_DIR", "/tmp/something")
    assert stt.get_current_variant() in {
        "small",
        "unknown",
        "medium",
        "large-v3",
        "large-v3-turbo",
        "remote",
    }


@pytest.mark.asyncio
async def test_llm_light_plus_formatter(monkeypatch, tmp_path):
    _settings(tmp_path, monkeypatch, ai_formatting_enabled=False)

    text = "to jest test test test bez kropki"
    out = await llm.format_text(text)
    assert isinstance(out, str)
    assert out[0].isupper()
    assert out.endswith(".")
    assert "test test test" not in out.lower()


@pytest.mark.asyncio
async def test_llm_remote_error_passthrough(monkeypatch, tmp_path):
    _settings(tmp_path, monkeypatch, ai_formatting_enabled=True, ai_provider="harmony")

    async def boom(text, assistive, settings):
        return None

    monkeypatch.setattr(llm, "_format_with_harmony", boom)
    out = await llm.format_text("abc")
    assert out.endswith(".")  # Light+ baseline fallback
