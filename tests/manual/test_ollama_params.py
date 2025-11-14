#!/usr/bin/env python3
"""Test exact Ollama parameters being sent"""

import json
import os

import pytest
import requests
from dotenv import load_dotenv

from tests.helpers.ollama import ensure_host_and_model, extract_response_text

load_dotenv()


# Test text
test_text = "To co ja mam na myśli to że jeśli włączymy transkrypcję"

# Build payload exactly as in llm.py
TEMPERATURE = float(os.environ.get("TEMPERATURE", "0.2"))
TOP_P = float(os.environ.get("TOP_P", "0.0"))
MAX_NEW_TOKENS = int(os.environ.get("MAX_NEW_TOKENS", "128"))

# For regular formatting (not assistive)
assistive = False
is_agent_mode = False
max_tokens = min(MAX_NEW_TOKENS, 512)  # Should be 512

FORMAT_PROMPT = (
    "TYLKO popraw błędy w tekście/kodzie. NIE wyjaśniaj. "
    "NIE twórz tabel. NIE dodawaj komentarzy. "
    "Zwróć WYŁĄCZNIE poprawiony tekst. Nic więcej."
)


def _build_payload(model: str) -> dict:
    return {
        "model": model,
        "system": FORMAT_PROMPT,
        "prompt": test_text,
        "stream": False,
        "options": {
            "temperature": TEMPERATURE,
            "top_p": TOP_P,
            "num_predict": max_tokens,
        },
    }


def test_payload_against_live_host():
    base, model = ensure_host_and_model()
    payload = _build_payload(model)
    print("=" * 60)
    print("OLLAMA PAYLOAD ANALYSIS")
    print("=" * 60)
    print(f"Model: {model}")
    print(f"Temperature: {TEMPERATURE}")
    print(f"TOP_P: {TOP_P}")
    print(f"MAX_NEW_TOKENS from env: {MAX_NEW_TOKENS}")
    print(f"Actual max_tokens used: {max_tokens}")
    print(f"System prompt: {FORMAT_PROMPT[:50]}...")
    print("=" * 60)
    print("Full payload:")
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    print("\n" + "=" * 60)
    print("TESTING WITH OLLAMA...")
    print("=" * 60)

    try:
        r = requests.post(f"{base}/api/generate", json=payload, timeout=30)
        r.raise_for_status()
    except requests.RequestException as exc:
        pytest.skip(f"Ollama request failed: {exc}")
    print(f"Raw Ollama response (first 200 chars): {r.text[:200]!r}")
    response = extract_response_text(r)
    if not response:
        pytest.skip("Ollama response is empty; adjust local model or re-run later")
