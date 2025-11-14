import io
import json
import wave

import pytest
from fastapi.testclient import TestClient

import vistascribe.backend as backend
from vistascribe.settings_store import reset_settings_for_tests


def make_wav_bytes(duration_ms=10, sr=16000):
    # Generate a tiny silent mono 16-bit WAV
    frames = int(sr * duration_ms / 1000)
    buf = io.BytesIO()
    with wave.open(buf, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sr)
        wf.writeframes(b"\x00\x00" * frames)
    return buf.getvalue()


@pytest.fixture()
def temp_settings(monkeypatch, tmp_path):
    """Write settings.json overrides and point the store at the tmp path."""

    def _write(data: dict):
        path = tmp_path / "settings.json"
        path.write_text(json.dumps(data), encoding="utf-8")
        monkeypatch.setenv("VISTASCRIBE_SETTINGS_PATH", str(path))
        reset_settings_for_tests()
        return path

    return _write


@pytest.fixture()
def app_client_and_spy(monkeypatch):
    # Ensure audio decode path uses WAV fallback regardless of mlx_audio availability
    monkeypatch.setattr(backend, "load_audio", None, raising=False)

    # Provide a dummy whisper model object
    dummy_model = object()
    monkeypatch.setattr(backend, "whisper_model", dummy_model, raising=False)

    # Spy for transcribe call correctness
    calls = {"args": None, "kwargs": None}

    class DummyWhisper:
        @staticmethod
        def transcribe(*args, **kwargs):
            # Record invocation for later assertions
            calls["args"] = args
            calls["kwargs"] = kwargs
            # Mimic mlx_whisper returning dict with text
            return {"text": "ok"}

    monkeypatch.setattr(backend, "whisper", DummyWhisper, raising=False)

    client = TestClient(backend.app)
    return client, calls, dummy_model


def test_transcribe_uses_correct_signature(app_client_and_spy):
    client, calls, dummy_model = app_client_and_spy

    wav = make_wav_bytes()
    files = {"audio": ("a.wav", io.BytesIO(wav), "audio/wav")}

    r = client.post("/transcribe", files=files)
    assert r.status_code == 200
    assert r.json().get("text") == "ok"

    # Ensure the backend passed file path positional arg + path metadata kwargs
    assert calls["args"] is not None
    assert len(calls["args"]) == 1
    final_path = calls["args"][0]
    assert isinstance(final_path, str)
    assert final_path.endswith(".wav")
    assert calls["kwargs"].get("path_or_hf_repo") == backend.WHISPER_DIR
    assert calls["kwargs"].get("initial_prompt")
    # whisper_model is still loaded (for health checks) but direct kwargs now reference model path
    assert "model" not in calls["kwargs"]


def test_format_endpoint_respects_toggle(monkeypatch, temp_settings):
    temp_settings({"ai_formatting_enabled": False})

    called = {"ai": False}

    async def fake_apply(text, assistive=False):  # pragma: no cover - monkeypatch
        called["ai"] = True
        return text + "::ai"

    monkeypatch.setattr(backend, "apply_ai_formatting", fake_apply)
    client = TestClient(backend.app)

    resp = client.post("/format", json={"text": "Hello"})
    assert resp.status_code == 200
    assert resp.json()["text"] == "Hello"
    assert called["ai"] is False


def test_format_endpoint_assistive_overrides_toggle(monkeypatch, temp_settings):
    temp_settings({"ai_formatting_enabled": False})

    async def fake_apply(text, assistive=False):
        assert assistive is True
        return f"AI::{text}"

    monkeypatch.setattr(backend, "apply_ai_formatting", fake_apply)
    client = TestClient(backend.app)

    resp = client.post("/format", json={"text": "Hello", "assistive": True})
    assert resp.status_code == 200
    assert resp.json()["text"] == "AI::Hello"


def test_stt_and_format_without_ai(monkeypatch, temp_settings, app_client_and_spy):
    temp_settings({"ai_formatting_enabled": False})

    async def boom(*_args, **_kwargs):  # pragma: no cover - should never run
        raise AssertionError("apply_ai_formatting should not run when disabled")

    monkeypatch.setattr(backend, "apply_ai_formatting", boom)

    client, _calls, _ = app_client_and_spy

    wav = make_wav_bytes()
    files = {"audio": ("a.wav", io.BytesIO(wav), "audio/wav")}

    resp = client.post("/stt_and_format", files=files)
    assert resp.status_code == 200
    assert resp.json()["text"] == "ok"


def test_stt_and_format_with_ai(monkeypatch, temp_settings, app_client_and_spy):
    temp_settings({"ai_formatting_enabled": True, "ai_provider": "harmony"})

    calls = {}

    async def fake_apply(text, assistive=False):
        calls["text"] = text
        calls["assistive"] = assistive
        return f"AI::{text}"

    monkeypatch.setattr(backend, "apply_ai_formatting", fake_apply)

    client, _calls, _ = app_client_and_spy

    wav = make_wav_bytes()
    files = {"audio": ("visit.wav", io.BytesIO(wav), "audio/wav")}

    resp = client.post(
        "/stt_and_format",
        data={"instruction": "Focus on meds"},
        files=files,
    )
    assert resp.status_code == 200
    assert resp.json()["text"] == "AI::ok"
    assert calls == {"text": "ok", "assistive": False}
