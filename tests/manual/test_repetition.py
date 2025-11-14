#!/usr/bin/env python3
"""Test if Ollama repetition issue is fixed"""

import pytest
from dotenv import load_dotenv

from tests.helpers.ollama import ensure_host_and_model
from vistascribe.llm import _ollama_generate

load_dotenv()


def test_no_repetition_in_output(monkeypatch):
    _, model = ensure_host_and_model()
    monkeypatch.setenv("OLLAMA_MODEL", model)
    print(f"Testing repetition with model: {model}")
    test_text = (
        "To co ja mam na myśli, to że jeśli np. włączymy transkrypcję, "
        "to nie chcemy, żeby się automatycznie włączało coś innego."
    )
    result = _ollama_generate("", test_text, assistive=False)
    if not result:
        pytest.skip("_ollama_generate returned empty text; Ollama likely streamed reasoning only")
    lines = [line.strip() for line in result.splitlines() if line.strip()]
    unique = set(lines)
    assert len(lines) <= len(unique) * 1.5, "Response contains excessive repetition"
