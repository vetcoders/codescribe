import json

import pytest

import vistascribe.llm as llm
from vistascribe.settings_store import reset_settings_for_tests


@pytest.mark.asyncio
async def test_ollama_backend_path(monkeypatch, tmp_path):
    settings_path = tmp_path / "settings.json"
    settings_path.write_text(
        json.dumps({"ai_formatting_enabled": True, "ai_provider": "ollama"}),
        encoding="utf-8",
    )
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(settings_path))
    reset_settings_for_tests()

    async def fake_formatter(text, assistive, settings):  # pragma: no cover - test helper
        return "FORMATTED_FROM_OLLAMA"

    monkeypatch.setattr(llm, "_format_with_ollama", fake_formatter)

    out = await llm.format_text("hello world")
    assert out == "FORMATTED_FROM_OLLAMA"
