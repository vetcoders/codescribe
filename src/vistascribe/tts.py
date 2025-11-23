"""
Optional TTS helper for VistaScribe (keeps tray lightweight).

This module does NOT import heavy ML at import time. It provides a thin
wrapper around the MLX-Audio CLI (preferred, since it runs in a separate
process) and a best-effort HTTP client for mlx_audio.server when available.

Usage (CLI fallback, recommended):
    from tts import say
    await say("Hi! This is a test.", play=True)

Env vars:
- TTS_SERVER_URL: if set, we'll try to POST to the server first, then
  fall back to the CLI if the endpoint is not available.
- TTS_MODEL: default model for CLI fallback (e.g. mlx-community/MetaVoice-1B-v0.1-mlx)

Notes:
- The CLI runs in a separate process, so MLX-Audio loads outside of the tray.
- If you enable HTTP mode, endpoints may differ by version; we try /tts and
  /api/tts with a JSON body {text, voice, speed, ...}. If both fail, we fall
  back to CLI.
"""

from __future__ import annotations

import asyncio
import functools
import logging
import os
import shlex
import sys
from collections.abc import Mapping
from typing import Any

DEFAULT_TTS_MODEL = os.environ.get("TTS_MODEL", "mlx-community/MetaVoice-1B-v0.1-mlx").strip()


def _http_post(url: str, json: Mapping[str, Any]) -> Mapping[str, Any]:
    import requests  # local import to avoid hard dep at import time

    resp = requests.post(url, json=json, timeout=60)
    resp.raise_for_status()
    try:
        return resp.json()  # type: ignore[return-value]
    except Exception:
        return {"ok": True}


async def _http_tts(text: str, opts: Mapping[str, Any]) -> bool:
    base = (os.environ.get("TTS_SERVER_URL", "").strip()).rstrip("/")
    if not base:
        return False
    payload = {"text": text}
    payload.update({k: v for k, v in opts.items() if v is not None})
    endpoints = ("/tts", "/api/tts")
    loop = asyncio.get_event_loop()
    for ep in endpoints:
        url = base + ep
        try:
            # Use partial to bind arguments correctly
            data = await loop.run_in_executor(None, functools.partial(_http_post, url, payload))
            if isinstance(data, dict):
                # Heuristic: consider success if no explicit error reported
                if not data.get("error"):
                    logging.info("TTS HTTP request succeeded at %s", url)
                    return True
        except Exception as e:
            logging.debug("TTS HTTP request failed at %s: %s", url, e)
    return False


def _build_cli_cmd(text: str, opts: Mapping[str, Any]) -> list[str]:
    model = (opts.get("model") or DEFAULT_TTS_MODEL).strip()
    voice = opts.get("voice")
    speed = opts.get("speed")
    gender = opts.get("gender")
    pitch = opts.get("pitch")
    lang = opts.get("lang_code")
    fmt = opts.get("audio_format") or "wav"
    prefix = opts.get("file_prefix") or "tts_out"
    play = bool(opts.get("play"))
    join = bool(opts.get("join_audio"))

    cmd = [
        sys.executable,
        "-m",
        "mlx_audio.tts.generate",
        "--model",
        model,
        "--file_prefix",
        prefix,
        "--audio_format",
        fmt,
    ]
    if voice:
        cmd += ["--voice", str(voice)]
    if speed is not None:
        cmd += ["--speed", str(speed)]
    if gender:
        cmd += ["--gender", str(gender)]
    if pitch is not None:
        cmd += ["--pitch", str(pitch)]
    if lang:
        cmd += ["--lang_code", str(lang)]
    if join:
        cmd += ["--join_audio"]
    if play:
        cmd += ["--play"]
    # Pass text via stdin (avoids shell escaping issues for large inputs)
    cmd += ["--prompt", "--text"] if False else []  # keep interface stable
    return cmd + ["--text", text]


async def _run_cli(cmd: list[str]) -> int:
    loop = asyncio.get_event_loop()

    def _runner() -> int:
        import subprocess

        try:
            # We pass text via "--text" argument; capture output for logging.
            proc = subprocess.run(cmd, capture_output=True, check=False)
            if proc.stdout:
                logging.debug("[TTS stdout] %s", proc.stdout.decode("utf-8", "ignore"))
            if proc.stderr:
                logging.debug("[TTS stderr] %s", proc.stderr.decode("utf-8", "ignore"))
            return proc.returncode
        except FileNotFoundError:
            return 127

    return await loop.run_in_executor(None, _runner)


async def say(
    text: str,
    *,
    model: str | None = None,
    voice: str | None = None,
    speed: float | None = None,
    gender: str | None = None,
    pitch: float | None = None,
    lang_code: str | None = None,
    file_prefix: str | None = None,
    audio_format: str | None = None,
    play: bool | None = None,
    join_audio: bool | None = None,
) -> bool:
    """Synthesize speech for the given text.

    Returns True if a synthesis path reported success, False otherwise.
    """
    text = (text or "").strip()
    if not text:
        return False

    opts: dict[str, Any] = {
        "model": model,
        "voice": voice,
        "speed": speed,
        "gender": gender,
        "pitch": pitch,
        "lang_code": lang_code,
        "file_prefix": file_prefix,
        "audio_format": audio_format,
        "play": play,
        "join_audio": join_audio,
    }

    # 1) Try HTTP server if configured
    tried_http = False
    if os.environ.get("TTS_SERVER_URL"):
        tried_http = True
        try:
            ok = await _http_tts(text, opts)
        except Exception as e:
            logging.debug("HTTP TTS attempt failed: %s", e)
            ok = False
        if ok:
            return True

    # 2) Fallback to CLI (separate process)
    cmd = _build_cli_cmd(text, opts)
    logging.info("Running TTS CLI: %s", " ".join(shlex.quote(p) for p in cmd))
    rc = await _run_cli(cmd)
    if rc == 0:
        return True

    # Final failure
    if tried_http:
        logging.warning("TTS failed via HTTP and CLI fallback.")
    else:
        logging.warning(
            "TTS CLI failed. Set TTS_SERVER_URL to use HTTP server, or "
            "ensure mlx-audio is installed."
        )
    return False
