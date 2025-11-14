#!/usr/bin/env python3
"""
backend.py — VistaScribe local backend (FastAPI)

Runs a lightweight HTTP server with preloaded MLX Whisper for STT and delegates
formatting to the shared Light+ + Harmony/Ollama pipeline (no local MLX-LM).

Endpoints:
- GET  /healthz
- POST /transcribe
- POST /format (expects Light+ text, applies AI enhancement when enabled)
- POST /stt_and_format
- POST /action (optional UI helpers)
- GET  /events (optional SSE state feed)
"""

from __future__ import annotations

# CRITICAL: Load .env FIRST before any other imports that read os.environ
from dotenv import load_dotenv

load_dotenv()

import asyncio
import base64
import contextlib
import io
import json
import logging
import os
import sys
import tempfile
from collections.abc import AsyncIterator, Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# MLX Whisper
try:
    import mlx_whisper as whisper  # type: ignore
    from mlx_whisper.load_models import load_model as load_whisper  # type: ignore
except Exception:  # pragma: no cover
    whisper = None  # type: ignore
    load_whisper = None  # type: ignore
from fastapi import (
    Body,
    FastAPI,
    File,
    Form,
    HTTPException,
    Request,
    UploadFile,
    WebSocket,
    WebSocketDisconnect,
)
from fastapi.responses import HTMLResponse, JSONResponse, StreamingResponse
from pydantic import BaseModel

# Optional audio decoding helper (API may vary by version)
try:  # mlx-audio >= 0.2.x sometimes does not expose `load`
    from mlx_audio import load as load_audio  # type: ignore
except Exception:  # pragma: no cover
    load_audio = None  # we'll fallback to WAV-only parsing

from .llm import apply_ai_formatting, run_chat_session
from .path_utils import normalize_model_path, repo_root
from .settings_store import get_settings

logger = logging.getLogger("vista-backend")

DEFAULT_STREAM_LANGUAGE = (
    os.environ.get("WHISPER_LANGUAGE") or os.environ.get("LANGUAGE") or ""
).strip().lower() or None

MAX_UPLOAD_MB = int(os.environ.get("BACKEND_MAX_UPLOAD_MB", "20"))
MAX_UPLOAD_BYTES = MAX_UPLOAD_MB * 1024 * 1024
_UPLOAD_CHUNK = 1024 * 1024
_ALLOWED_AUDIO_EXTS = {".wav", ".mp3", ".m4a", ".flac", ".ogg", ".webm"}
_ALLOWED_AUDIO_MIME = {
    "audio/wav",
    "audio/x-wav",
    "audio/mpeg",
    "audio/mp3",
    "audio/webm",
    "audio/ogg",
    "audio/flac",
    "audio/x-m4a",
    "audio/mp4",
}

MEDICAL_PROMPT = (
    "Polski tekst weterynaryjny: diagnoza chorób zwierząt, objawy kliniczne, "
    "leczenie farmakologiczne, badania laboratoryjne, RTG, USG, szczepienia, "
    "odrobaczanie, wizyty kontrolne, wyniki badań krwi, temperatura ciała, "
    "receptury leków, dawkowanie, rokowanie, zalecenia pielęgnacyjne."
)


@dataclass
class StreamSession:
    """Mutable state for streaming (JSON or WebSocket) sessions."""

    language: str | None = DEFAULT_STREAM_LANGUAGE
    sample_rate: int = 16000
    encoding: str = "pcm16"
    buffer: bytearray = field(default_factory=bytearray)


# Feature guards: disable optional endpoints by default unless explicitly enabled
_ACTIONS_ENABLED = os.environ.get("BACKEND_ACTIONS", "0").strip().lower() not in {
    "0",
    "false",
    "no",
    "off",
}
_EVENTS_ENABLED = os.environ.get("BACKEND_EVENTS", "0").strip().lower() not in {
    "0",
    "false",
    "no",
    "off",
}
_EVENT_HEARTBEAT_SECONDS = int(os.environ.get("BACKEND_EVENT_HEARTBEAT", "20"))

app = FastAPI(title="VistaScribe-backend")

# --- Config / Model load ---
REPO_ROOT = repo_root()


def _asset_path(name: str) -> Path:
    candidates = [
        REPO_ROOT / "src" / "vistascribe" / "assets" / name,
        REPO_ROOT / "assets" / name,
        REPO_ROOT.parent / "assets" / name,
        REPO_ROOT.parent / "Resources" / "assets" / name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return candidates[0]


# Whisper path selection (prefer small for "survival" mode; configurable via env)
_variant = os.environ.get("WHISPER_VARIANT", "small").strip().lower()  # Default: small!
if not os.environ.get("WHISPER_DIR"):
    candidates: list[str] = []
    if _variant in {"medium", "large-v3-turbo", "large-v3", "small", "small-mlx"}:
        candidates.append(os.path.join(REPO_ROOT, "models", f"whisper-{_variant}"))
        candidates.append(os.path.join(REPO_ROOT, f"whisper-{_variant}"))
    else:
        # Default search: prefer smaller models first (faster downloads on first run)
        for v in ("small", "medium", "large-v3", "large-v3-turbo", "small-mlx"):
            candidates.append(os.path.join(REPO_ROOT, "models", f"whisper-{v}"))
            candidates.append(os.path.join(REPO_ROOT, f"whisper-{v}"))
    _default_whisper_path = next(
        (c for c in candidates if os.path.isdir(c)),
        os.path.join(REPO_ROOT, "models", "whisper-small"),  # Fallback to small!
    )
else:
    _default_whisper_path = os.environ["WHISPER_DIR"]

WHISPER_DIR = normalize_model_path(_default_whisper_path)

try:
    whisper_model = load_whisper(WHISPER_DIR)
    logger.info(f"MLX Whisper model loaded from: {WHISPER_DIR}")
except Exception as e:
    logger.error(f"Failed to load MLX Whisper model: {e}")
    whisper_model = None

logger.info("VistaScribe backend ready (Light+ baseline, Harmony/Ollama AI formatting).")

# --- State broadcasting (SSE) ---
_state: str = "idle"
_subs: list[asyncio.Queue[str]] = []

# Serialize MLX Whisper calls to avoid Metal command buffer conflicts
_whisper_lock: asyncio.Lock | None = None


async def _get_whisper_lock() -> asyncio.Lock:
    """Get or create the Whisper serialization lock."""
    global _whisper_lock
    if _whisper_lock is None:
        _whisper_lock = asyncio.Lock()
    return _whisper_lock


def _pcm16le_to_wav_bytes(pcm: bytes, sample_rate: int = 16000, channels: int = 1) -> bytes:
    import wave

    buf = io.BytesIO()
    with wave.open(buf, "wb") as wf:
        wf.setnchannels(channels)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm)
    return buf.getvalue()


async def _run_whisper_transcription(audio_path: str, *, language: str | None) -> dict[str, Any]:
    """Serialize whisper transcription to avoid Metal command buffer conflicts."""

    if whisper is None:
        raise RuntimeError("Whisper bindings not available")

    lock = await _get_whisper_lock()

    async with lock:

        def _do_transcribe():
            try:
                return whisper.transcribe(  # type: ignore[attr-defined]
                    audio_path,
                    path_or_hf_repo=WHISPER_DIR,
                    verbose=True,
                    language=language,
                    condition_on_previous_text=False,
                    initial_prompt=MEDICAL_PROMPT,
                )
            except TypeError:
                return _legacy_whisper_transcribe(
                    audio_path,
                    language=language,
                )

        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(None, _do_transcribe)


async def _transcribe_audio_file(path: str, *, language: str | None) -> dict[str, Any]:
    """Wrapper that runs whisper and returns the raw dict response."""

    result = await _run_whisper_transcription(path, language=language)
    return result


def _legacy_whisper_transcribe(audio_path: str, *, language: str | None) -> dict[str, Any]:
    """Fallback for older `mlx_whisper.transcribe(model, samples, sr, **kw)`."""

    if whisper_model is None:
        raise RuntimeError("Whisper model not loaded")

    try:
        from mlx_whisper.utils.audio import load_audio  # type: ignore
    except Exception as exc:  # pragma: no cover - depends on env
        raise RuntimeError("mlx_whisper audio loader unavailable") from exc

    samples, sample_rate = load_audio(audio_path)
    if samples.ndim > 1:
        # downmix to mono
        samples = samples.mean(axis=1)

    kwargs: dict[str, Any] = {
        "condition_on_previous_text": False,
        "initial_prompt": MEDICAL_PROMPT,
    }
    if language:
        kwargs["language"] = language

    return whisper.transcribe(  # type: ignore[attr-defined]
        whisper_model,
        samples,
        sample_rate,
        **kwargs,
    )


async def _transcribe_stream_buffer(session: StreamSession) -> dict[str, Any]:
    """Transcribe the accumulated streaming buffer and clear it."""

    if not session.buffer:
        return {"text": ""}

    if session.encoding in {"pcm16", "pcm_s16le"}:
        wav_bytes = _pcm16le_to_wav_bytes(bytes(session.buffer), sample_rate=session.sample_rate)
    elif session.encoding in {"wav", "audio/wav"}:
        wav_bytes = bytes(session.buffer)
    else:
        raise ValueError(f"unsupported encoding: {session.encoding}")

    with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
        tmp.write(wav_bytes)
        tmp_path = tmp.name

    try:
        result = await _transcribe_audio_file(tmp_path, language=session.language)
    finally:
        try:
            os.remove(tmp_path)
        except Exception:
            pass

    session.buffer.clear()
    return result


async def _stream_send_transcript(
    session: StreamSession,
    send: Callable[[dict[str, Any]], Awaitable[None]],
) -> None:
    result = await _transcribe_stream_buffer(session)
    text = (result.get("text") or result.get("transcription") or "").strip()
    await send(
        {
            "type": "transcript.final",
            "text": text,
            "duration_ms": result.get("duration_ms"),
        }
    )


async def _handle_stream_message(
    msg: dict[str, Any],
    session: StreamSession,
    send: Callable[[dict[str, Any]], Awaitable[None]],
) -> bool:
    """Process a streaming control/chunk message. Returns True to end session."""

    mtype = msg.get("type")
    if mtype == "chunk":
        b64 = msg.get("audio_base64")
        if not isinstance(b64, str):
            await send({"type": "error", "message": "audio_base64 required"})
            return False
        try:
            chunk = base64.b64decode(b64)
        except Exception:
            await send({"type": "error", "message": "invalid base64"})
            return False
        sample_rate_value = msg.get("sample_rate")
        if sample_rate_value is not None:
            try:
                session.sample_rate = int(sample_rate_value)
            except (TypeError, ValueError):
                session.sample_rate = 16000
        if msg.get("encoding"):
            session.encoding = str(msg.get("encoding")).lower()
        session.buffer.extend(chunk)
        await send({"type": "ack", "received_bytes": len(session.buffer)})
        if msg.get("last") is True:
            try:
                await _stream_send_transcript(session, send)
            except ValueError as exc:
                await send({"type": "error", "message": str(exc)})
        return False

    if mtype == "flush":
        try:
            await _stream_send_transcript(session, send)
        except ValueError as exc:
            await send({"type": "error", "message": str(exc)})
        return False

    if mtype == "end":
        if session.buffer:
            try:
                await _stream_send_transcript(session, send)
            except ValueError as exc:
                await send({"type": "error", "message": str(exc)})
                session.buffer.clear()
        return True

    if mtype == "set":
        if "language" in msg:
            lang = str(msg.get("language") or "").strip().lower()
            session.language = lang or None
        if "sample_rate" in msg:
            sample_rate_value = msg.get("sample_rate")
            if sample_rate_value is not None:
                try:
                    session.sample_rate = int(sample_rate_value)
                except (TypeError, ValueError):
                    session.sample_rate = 16000
        if "encoding" in msg:
            session.encoding = str(msg.get("encoding") or "pcm16").lower()
        await send(
            {
                "type": "ack",
                "language": session.language,
                "sample_rate": session.sample_rate,
                "encoding": session.encoding,
            }
        )
        return False

    await send({"type": "error", "message": f"unknown type: {mtype}"})
    return False


async def _iter_ndjson_lines(request: Request) -> AsyncIterator[str]:
    """Stream newline-delimited JSON payloads from the request body."""

    buffer = ""
    async for chunk in request.stream():
        buffer += chunk.decode("utf-8", "replace")
        while True:
            idx = buffer.find("\n")
            if idx == -1:
                break
            line, buffer = buffer[:idx], buffer[idx + 1 :]
            line = line.strip()
            if line:
                yield line
    tail = buffer.strip()
    if tail:
        yield tail


async def _broadcast_state():
    msg = json.dumps({"state": _state})
    for q in list(_subs):
        try:
            await q.put(msg)
        except Exception:
            pass


@app.get("/healthz")
async def healthz():
    ok = whisper_model is not None
    settings = get_settings()
    return {
        "ok": ok,
        "state": _state,
        "ai": {
            "enabled": settings.ai_formatting_enabled,
            "provider": settings.ai_provider,
        },
    }


class FormatRequest(BaseModel):
    text: str
    instruction: str | None = None
    assistive: bool = False  # Add assistive mode flag


class ChatDemoMessage(BaseModel):
    role: str
    content: str


class ChatDemoRequest(BaseModel):
    messages: list[ChatDemoMessage]


@app.post("/stream/transcribe")
async def transcribe_stream_endpoint(request: Request):
    """NDJSON-over-HTTP helper (messages echoed back in the response)."""

    if whisper_model is None:
        return JSONResponse(status_code=500, content={"error": "Whisper not initialized"})

    session = StreamSession()
    events: list[str] = []

    async def send(payload: dict[str, Any]) -> None:
        events.append(json.dumps(payload, ensure_ascii=False))

    await send({"type": "hello", "protocol": "stt-jsonl-v1"})

    try:
        async for line in _iter_ndjson_lines(request):
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                await send({"type": "error", "message": "invalid JSON"})
                continue
            should_close = await _handle_stream_message(msg, session, send)
            if should_close:
                break
    except Exception as exc:  # pragma: no cover - network edge cases
        await send({"type": "error", "message": str(exc)})
    finally:
        await send({"type": "stream.closed"})

    body = "\n".join(events) + "\n"
    return StreamingResponse(iter([body]), media_type="application/x-ndjson")


@app.post("/transcribe")
async def transcribe_endpoint(audio: UploadFile = File(...)):  # noqa: B008  # FastAPI pattern
    if whisper_model is None:
        return JSONResponse(status_code=500, content={"error": "Whisper not initialized"})

    import subprocess
    import tempfile

    temp_path = None
    converted_path = None
    try:
        filename = (audio.filename or "audio.wav").strip()
        suffix = os.path.splitext(filename)[1].lower() or ".wav"
        if suffix not in _ALLOWED_AUDIO_EXTS:
            raise HTTPException(status_code=415, detail="Unsupported audio type")
        if audio.content_type and audio.content_type.lower() not in _ALLOWED_AUDIO_MIME:
            raise HTTPException(status_code=415, detail="Unsupported audio MIME type")
        raw_header_len = audio.headers.get("content-length") if audio.headers else None
        if raw_header_len:
            try:
                header_len = int(raw_header_len)
            except ValueError:
                header_len = None
            if header_len is not None and header_len > MAX_UPLOAD_BYTES:
                raise HTTPException(status_code=413, detail="Audio file too large")

        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tf:
            total = 0
            while True:
                chunk = await audio.read(_UPLOAD_CHUNK)
                if not chunk:
                    break
                total += len(chunk)
                if total > MAX_UPLOAD_BYTES:
                    raise HTTPException(status_code=413, detail="Audio file too large")
                tf.write(chunk)
            temp_path = tf.name

        # Convert to WAV if needed (using ffmpeg for webm/mp3/etc)
        if suffix != ".wav":
            converted_path = temp_path.replace(suffix, ".wav")
            try:
                result = subprocess.run(
                    ["ffmpeg", "-i", temp_path, "-ar", "16000", "-ac", "1", "-y", converted_path],
                    capture_output=True,
                    timeout=30,
                )
            except FileNotFoundError as exc:
                raise HTTPException(
                    status_code=415,
                    detail="ffmpeg is not installed. Install it (brew install ffmpeg) or upload WAV/FLAC audio.",
                ) from exc
            if result.returncode != 0:
                raise RuntimeError(f"ffmpeg conversion failed: {result.stderr.decode()[:200]}")
            final_path = converted_path
        else:
            final_path = temp_path

        # Language preference (support WHISPER_LANGUAGE env var like stt.py)
        lang = os.environ.get("WHISPER_LANGUAGE", "").strip().lower() or None

        transcription = await _transcribe_audio_file(final_path, language=lang)
        text = (transcription.get("text") or "").strip()

        # Log detected language for debugging
        if isinstance(transcription, dict):
            detected = transcription.get("language", "unknown")
            logger.info(f"Whisper detected language: {detected}")

        return {"text": text}
    except Exception as e:
        logger.exception("Transcription failed")
        return JSONResponse(status_code=500, content={"error": str(e)})
    finally:
        # Clean up temp files
        for p in [temp_path, converted_path]:
            if p and os.path.exists(p):
                try:
                    os.remove(p)
                except Exception:
                    pass


@app.post("/format")
async def format_endpoint(req: FormatRequest):
    """Apply AI enhancement. Incoming text is already Light+ formatted."""

    text = req.text or ""
    if not text:
        return {"text": ""}

    settings = get_settings()
    if not req.assistive and not settings.ai_formatting_enabled:
        return {"text": text}

    formatted = await apply_ai_formatting(text, assistive=req.assistive)
    return {"text": formatted}


@app.post("/stt_and_format")
async def stt_and_format(
    audio: UploadFile = File(...),  # noqa: B008  # FastAPI pattern
    instruction: str | None = Form(None),
):
    t = await transcribe_endpoint(audio)
    if isinstance(t, JSONResponse):
        return t
    txt = t.get("text", "")
    settings = get_settings()
    if not settings.ai_formatting_enabled:
        return {"text": txt}
    f = await format_endpoint(FormatRequest(text=txt, instruction=instruction))
    return f


@app.post("/demo/chat")
async def chat_demo_endpoint(req: ChatDemoRequest):
    if not req.messages:
        raise HTTPException(status_code=422, detail="messages are required")
    try:
        text = await run_chat_session(
            [
                {"role": msg.role, "content": msg.content}
                for msg in req.messages
                if (msg.content or "").strip()
            ],
            settings=get_settings(),
        )
    except HTTPException:
        raise
    except Exception as exc:
        logger.exception("Chat demo failed: %s", exc)
        raise HTTPException(status_code=500, detail=str(exc)) from exc
    return {"text": text}


@app.websocket("/ws/transcribe")
async def websocket_transcribe(ws: WebSocket):
    if whisper_model is None:
        await ws.accept()
        await ws.send_text(json.dumps({"type": "error", "message": "Whisper not initialized"}))
        await ws.close()
        return

    await ws.accept()
    session = StreamSession()

    async def send(payload: dict[str, Any]) -> None:
        await ws.send_text(json.dumps(payload, ensure_ascii=False))

    await send({"type": "hello", "protocol": "stt-ws-v1"})

    try:
        while True:
            raw = await ws.receive_text()
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                await send({"type": "error", "message": "invalid JSON"})
                continue
            should_close = await _handle_stream_message(msg, session, send)
            if should_close:
                await ws.close()
                break
    except WebSocketDisconnect:
        return
    except Exception as exc:
        await send({"type": "error", "message": str(exc)})
        with contextlib.suppress(Exception):
            await ws.close()


@app.post("/action")
async def action(payload: dict = Body(...)):  # noqa: B008  # FastAPI pattern
    if not _ACTIONS_ENABLED:
        return JSONResponse(status_code=404, content={"error": "/action disabled"})
    global _state
    action = (payload.get("action") or "").strip().lower()
    mapping = {
        "activate": "listening",
        "idle": "idle",
        "mute": "muted",
        "thinking": "thinking",
        "success": "success",
        "error": "error",
    }
    _state = mapping.get(action, _state)
    await _broadcast_state()
    return {"ok": True, "state": _state}


@app.get("/events")
async def events():
    if not _EVENTS_ENABLED:
        return JSONResponse(status_code=404, content={"error": "/events disabled"})
    q: asyncio.Queue[str] = asyncio.Queue()
    _subs.append(q)

    async def gen() -> AsyncIterator[str]:
        try:
            # initial
            yield f"data: {json.dumps({'state': _state})}\n\n"
            while True:
                try:
                    data = await asyncio.wait_for(q.get(), timeout=_EVENT_HEARTBEAT_SECONDS)
                except TimeoutError:  # asyncio.wait_for propagates TimeoutError
                    yield ":ping\n\n"
                    continue
                yield f"data: {data}\n\n"
        except asyncio.CancelledError:
            pass
        finally:
            try:
                _subs.remove(q)
            except ValueError:
                pass

    return StreamingResponse(gen(), media_type="text/event-stream")


@app.get("/version")
async def version():
    """Return basic version and readiness info for integrations."""
    try:
        from importlib.metadata import version as _pkg_version  # type: ignore

        pkg_ver = _pkg_version("VistaScribe")
    except Exception:
        pkg_ver = os.environ.get("APP_VERSION", "dev")

    # Lazy checks to avoid importing heavy libs
    try:
        import importlib

        mlx_audio_ok = importlib.util.find_spec("mlx_audio") is not None  # type: ignore[attr-defined]
        mlx_lm_ok = importlib.util.find_spec("mlx_lm") is not None  # type: ignore[attr-defined]
        mlx_whisper_ok = importlib.util.find_spec("mlx_whisper") is not None  # type: ignore[attr-defined]
    except Exception:
        mlx_audio_ok = mlx_lm_ok = mlx_whisper_ok = False

    return {
        "version": pkg_ver,
        "mlx": {
            "audio": bool(mlx_audio_ok),
            "lm": bool(mlx_lm_ok),
            "whisper": bool(mlx_whisper_ok),
        },
        "ready": {
            "whisper": whisper_model is not None,
            "llm": get_settings().ai_formatting_enabled,
        },
        "state": _state,
    }


@app.get("/ready")
async def ready():
    """Alias for /healthz to support conventional readiness probes."""
    return await healthz()


@app.get("/tester")
async def tester():
    """Serve the Voice & Chat Lab (spectrogram + chat)."""
    try:
        path = _asset_path("voice_chat_lab.html")
        with open(path, encoding="utf-8") as f:
            html = f.read()
        return HTMLResponse(content=html, status_code=200)
    except Exception as e:
        return HTMLResponse(content=f"<h1>Error</h1><pre>{e}</pre>", status_code=500)


if __name__ == "__main__":
    from .vistascribe_server import main as server_main

    sys.exit(server_main())
