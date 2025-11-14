"""Shared helpers for Ollama-dependent tests."""

from __future__ import annotations

import json
import os

import pytest
import requests


def ensure_host_and_model() -> tuple[str, str]:
    host = (os.environ.get("OLLAMA_HOST") or "http://127.0.0.1:11434").rstrip("/")
    try:
        resp = requests.get(f"{host}/api/tags", timeout=2)
        resp.raise_for_status()
    except Exception as exc:  # pragma: no cover - requires running daemon
        pytest.skip(f"Ollama host unavailable: {exc}")
    data = resp.json() if resp.content else {}
    models = [item.get("name") for item in data.get("models", []) if item.get("name")]
    env_model = (os.environ.get("OLLAMA_MODEL") or "").strip()
    model = env_model if env_model in models else (models[0] if models else None)
    if not model:
        pytest.skip("Ollama host running but no models are installed")
    return host, model


def extract_response_text(resp: requests.Response) -> str:
    raw = resp.text or ""
    if not raw.strip():
        return ""
    pieces: list[str] = []
    lines = [line.strip() for line in raw.splitlines() if line.strip()]
    if not lines:
        lines = [raw.strip()]
    for line in lines:
        try:
            data = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(data, dict):
            continue
        text = data.get("response") or data.get("output") or ""
        if text:
            pieces.append(text)
    if not pieces:
        return ""
    return "".join(pieces).strip()


__all__ = ["ensure_host_and_model", "extract_response_text"]
