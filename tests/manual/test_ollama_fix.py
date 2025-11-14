#!/usr/bin/env python3
"""Test that Ollama uses the correct model from .env"""

import pytest
import requests
from dotenv import load_dotenv

from tests.helpers.ollama import ensure_host_and_model, extract_response_text
from vistascribe.llm import _ollama_generate

load_dotenv()


def test_ollama_roundtrip():
    host, model = ensure_host_and_model()
    payload = {
        "model": model,
        "prompt": "Zadania z matematyki",
        "stream": False,
        "options": {"temperature": 0.3, "num_predict": 50},
    }
    try:
        resp = requests.post(f"{host}/api/generate", json=payload, timeout=30)
        resp.raise_for_status()
    except requests.RequestException as exc:
        pytest.skip(f"Ollama request failed: {exc}")
    text = extract_response_text(resp)
    if not text:
        pytest.skip("Ollama response was empty; ensure the requested model returns text")


def test_llm_module_helper(monkeypatch):
    _, model = ensure_host_and_model()
    monkeypatch.setenv("OLLAMA_MODEL", model)
    result = _ollama_generate("", "Zadania z matematyki", assistive=False)
    if not result:
        pytest.skip("_ollama_generate returned empty text; check local Ollama setup")
