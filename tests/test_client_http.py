"""HTTP client end-to-end tests covering transcribe/format flows."""

from __future__ import annotations

import wave
from contextlib import asynccontextmanager
from pathlib import Path

import pytest
from aiohttp import web
from aiohttp.test_utils import unused_port

import vistascribe.client as client
from vistascribe.settings_store import VistaSettings


def _write_silence(path: Path, duration_ms: int = 20, sr: int = 16_000) -> None:
    frames = int(sr * duration_ms / 1000)
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sr)
        wf.writeframes(b"\x00\x00" * frames)


@asynccontextmanager
async def _run_server(register_routes):
    app = web.Application()
    register_routes(app)
    runner = web.AppRunner(app)
    await runner.setup()
    port = unused_port()
    site = web.TCPSite(runner, "127.0.0.1", port)
    await site.start()
    try:
        yield f"http://127.0.0.1:{port}"
    finally:
        await runner.cleanup()


@pytest.mark.asyncio
async def test_transcribe_http_success(monkeypatch, tmp_path):
    clip = tmp_path / "clip.wav"
    _write_silence(clip)

    async def handle_transcribe(request: web.Request):
        data = await request.post()
        field = data["audio"]
        payload = field.file.read()
        assert field.filename == "clip.wav"
        assert payload.startswith(b"RIFF")
        return web.json_response({"text": "server-ok"})

    async with _run_server(
        lambda app: app.router.add_post("/transcribe", handle_transcribe)
    ) as base_url:

        async def _resolve(_: bool = False):
            return base_url

        monkeypatch.setattr(client, "resolve_server_url", _resolve)
        out = await client.transcribe_http(str(clip))
        assert out == "server-ok"


@pytest.mark.asyncio
async def test_transcribe_http_missing_server(monkeypatch, tmp_path):
    async def _resolve(*_args, **_kwargs):  # pragma: no cover - simple stub
        return None

    monkeypatch.setattr(client, "resolve_server_url", _resolve)
    clip = tmp_path / "clip.wav"
    _write_silence(clip)
    out = await client.transcribe_http(str(clip))
    assert out is None


@pytest.mark.asyncio
async def test_format_text_http_ai_disabled_returns_baseline(monkeypatch):
    monkeypatch.setattr(client, "apply_light_plus", lambda text: f"LP::{text.strip()}")
    monkeypatch.setattr(client, "get_settings", lambda: VistaSettings(ai_formatting_enabled=False))

    async def _fail(*_args, **_kwargs):  # pragma: no cover - guarded path
        raise AssertionError("should not query server when AI disabled")

    monkeypatch.setattr(client, "resolve_server_url", _fail)
    out = await client.format_text_http("  hello  ")
    assert out == "LP::hello"


@pytest.mark.asyncio
async def test_format_text_http_ai_enabled_hits_backend(monkeypatch):
    monkeypatch.setattr(client, "apply_light_plus", lambda text: f"LP::{text.strip()}")
    monkeypatch.setattr(
        client,
        "get_settings",
        lambda: VistaSettings(ai_formatting_enabled=True, ai_provider="harmony"),
    )

    async def handle_format(request: web.Request):
        payload = await request.json()
        assert payload == {"text": "LP::note", "assistive": True}
        return web.json_response({"text": "AI::note"})

    async with _run_server(lambda app: app.router.add_post("/format", handle_format)) as base_url:

        async def _resolve(*_args, **_kwargs):
            return base_url

        monkeypatch.setattr(client, "resolve_server_url", _resolve)
        out = await client.format_text_http(" note ", assistive=True)
        assert out == "AI::note"


@pytest.mark.asyncio
async def test_format_text_http_falls_back_on_server_error(monkeypatch):
    monkeypatch.setattr(client, "apply_light_plus", lambda text: "LP::boom")
    monkeypatch.setattr(
        client,
        "get_settings",
        lambda: VistaSettings(ai_formatting_enabled=True, ai_provider="harmony"),
    )

    async def handle_format(_request: web.Request):
        return web.Response(status=500)

    async with _run_server(lambda app: app.router.add_post("/format", handle_format)) as base_url:

        async def _resolve(*_args, **_kwargs):
            return base_url

        monkeypatch.setattr(client, "resolve_server_url", _resolve)
        out = await client.format_text_http("boom")
        assert out == "LP::boom"
