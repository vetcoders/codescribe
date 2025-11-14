import asyncio
import importlib
import json

from vistascribe.formatting import apply_light_plus
from vistascribe.settings_store import reset_settings_for_tests


def test_format_passthrough_when_disabled(monkeypatch, tmp_path):
    path = tmp_path / "settings.json"
    path.write_text(json.dumps({"ai_formatting_enabled": False}), encoding="utf-8")
    monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
    reset_settings_for_tests()

    import vistascribe.llm as llm_mod

    importlib.reload(llm_mod)

    sample = "to jest test"
    out = asyncio.run(llm_mod.format_text(sample))
    assert out == apply_light_plus(sample)
