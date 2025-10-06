import asyncio
import importlib


def test_format_passthrough_when_disabled(monkeypatch):
    # Ensure formatting is disabled and the function returns input unchanged
    monkeypatch.setenv("FORMAT_ENABLED", "0")
    # Avoid accidental OpenAI/backend usage
    monkeypatch.setenv("FORMAT_BACKEND", "local")

    import llm as llm_mod

    importlib.reload(llm_mod)

    sample = "to jest test"
    out = asyncio.run(llm_mod.format_text(sample))
    assert out == sample
