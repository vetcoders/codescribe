#!/usr/bin/env python3
"""
backend.py — CodeScribe local backend (FastAPI)

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

from .path_utils import repo_root, user_data_root

# Load .env files: repo defaults first, then user overrides
_repo_env = repo_root() / ".env"
if _repo_env.exists():
    load_dotenv(dotenv_path=_repo_env)
else:
    load_dotenv()

# Load user data .env (~/.CodeScribe/.env) - overrides repo settings
_user_env = user_data_root() / ".env"
if _user_env.exists():
    load_dotenv(dotenv_path=_user_env, override=True)

import asyncio
import base64
import contextlib
import io
import json
import logging
import os
import sys
import tempfile
import time
import uuid
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
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

# Optional audio decoding helper (API may vary by version)
try:  # mlx-audio >= 0.2.x sometimes does not expose `load`
    from mlx_audio import load as load_audio  # type: ignore
except Exception:  # pragma: no cover
    load_audio = None  # we'll fallback to WAV-only parsing

from .formatting import apply_light_plus
from .formatting.vocabulary import (
    append_lexicon_entries,
    lexicon_path_for_topic,
    load_lexicon_entries,
    reload_lexicon,
    sanitize_topic,
)
from .llm import _harmony_base_url, apply_ai_formatting, run_chat_session
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

    session_id: str = field(default_factory=lambda: uuid.uuid4().hex)
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

app = FastAPI(title="CodeScribe-backend")

# --- Config / Model load ---
REPO_ROOT = repo_root()


def _asset_path(name: str) -> Path:
    candidates = [
        REPO_ROOT / "src" / "codescribe" / "assets" / name,
        REPO_ROOT / "assets" / name,
        REPO_ROOT.parent / "assets" / name,
        REPO_ROOT.parent / "Resources" / "assets" / name,
    ]
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return candidates[0]


LAB_ASSETS_DIR = _asset_path("lab")
if LAB_ASSETS_DIR.exists():
    app.mount(
        "/lab-assets",
        StaticFiles(directory=str(LAB_ASSETS_DIR), html=False),
        name="lab-assets",
    )
else:
    logger.warning("Voice & Chat Lab assets missing at %s", LAB_ASSETS_DIR)


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

logger.info("CodeScribe backend ready (Light+ baseline, Harmony/Ollama AI formatting).")

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


def _probe_wav_stats(path: str, *, fallback_rate: int = 16000) -> dict[str, float | int]:
    """Return basic audio stats for logging."""

    info: dict[str, float | int] = {"bytes": 0, "duration_sec": 0.0, "frames": 0, "sample_rate": 0}
    try:
        info["bytes"] = os.path.getsize(path)
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)
    try:
        import wave

        with wave.open(path, "rb") as wf:
            frames = wf.getnframes()
            rate = wf.getframerate() or fallback_rate
            info["frames"] = frames
            info["sample_rate"] = rate
            info["duration_sec"] = frames / float(rate or 1)
    except Exception:
        # best-effort only; leave defaults
        info.setdefault("sample_rate", fallback_rate)
    return info


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
                    # Anti-hallucination filters (improves transcription quality)
                    compression_ratio_threshold=2.0,  # Lower = stricter (default 2.4)
                    no_speech_threshold=0.5,  # Higher = stricter (default 0.6)
                    logprob_threshold=-0.5,  # Higher = stricter (default -1.0)
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
        # Anti-hallucination filters (improves transcription quality)
        "compression_ratio_threshold": 2.0,  # Lower = stricter (default 2.4)
        "no_speech_threshold": 0.5,  # Higher = stricter (default 0.6)
        "logprob_threshold": -0.5,  # Higher = stricter (default -1.0)
    }
    if language:
        kwargs["language"] = language

    return whisper.transcribe(  # type: ignore[attr-defined]
        whisper_model,
        samples,
        sample_rate,
        **kwargs,
    )


def _session_id_from_request(request: Request | None) -> str:
    if request:
        try:
            header_sid = request.headers.get("X-Session-ID")
            if header_sid:
                return header_sid
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)
    return uuid.uuid4().hex


async def _transcribe_stream_buffer(session: StreamSession) -> dict[str, Any]:
    """Transcribe the accumulated streaming buffer and clear it."""

    if not session.buffer:
        return {"text": ""}

    buf_len = len(session.buffer)
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
        frames = int(len(wav_bytes) / 2)
        duration_sec = frames / float(session.sample_rate or 1)
        result["_buffer_bytes"] = buf_len
        result["_buffer_frames"] = frames
        result["_buffer_duration_sec"] = duration_sec
    finally:
        try:
            os.remove(tmp_path)
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

    session.buffer.clear()
    return result


async def _stream_send_transcript(
    session: StreamSession,
    send: Callable[[dict[str, Any]], Awaitable[None]],
) -> None:
    result = await _transcribe_stream_buffer(session)
    text = (result.get("text") or result.get("transcription") or "").strip()
    logger.info(
        "stream transcript session=%s lang=%s bytes=%s frames=%s dur=%.3fs out_chars=%s",
        session.session_id,
        session.language or "auto",
        result.get("_buffer_bytes"),
        result.get("_buffer_frames"),
        result.get("_buffer_duration_sec"),
        len(text),
    )
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
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)


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


@app.get("/lab/config")
async def lab_config(request: Request):
    """Expose default Lab endpoints from .env for the Lab UI with override slots."""

    settings = get_settings()
    base_http = str(request.base_url).rstrip("/")
    protocol = "wss" if request.url.scheme == "https" else "ws"
    host = request.url.hostname or "localhost"
    port = f":{request.url.port}" if request.url.port else ""
    default_ws = f"{protocol}://{host}{port}/ws/transcribe"

    harmony_responses = None
    try:
        harmony_base = _harmony_base_url()
        harmony_responses = f"{harmony_base}/responses"
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)

    return {
        "stt_upload_url": f"{base_http}/transcribe",
        "stt_and_format_url": f"{base_http}/stt_and_format",
        "stt_ndjson_url": f"{base_http}/stream/transcribe",
        "stt_ws_url": default_ws,
        "responses_url": harmony_responses,
        "ai_provider": settings.ai_provider,
        "harmony_model": os.environ.get("HARMONY_MODEL") or None,
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
    logger.info(
        "stream-jsonl session=%s lang=%s sample_rate=%s encoding=%s",
        session.session_id,
        session.language,
        session.sample_rate,
        session.encoding,
    )
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
async def transcribe_endpoint(  # noqa: B008  # FastAPI pattern
    request: Request,
    audio: UploadFile = File(...),  # noqa: B008  # FastAPI requires call here
):
    if whisper_model is None:
        return JSONResponse(status_code=500, content={"error": "Whisper not initialized"})

    import subprocess
    import tempfile

    session_id = _session_id_from_request(request)
    start_time = time.perf_counter()
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
        stats = _probe_wav_stats(final_path, fallback_rate=16000)
        elapsed = time.perf_counter() - start_time
        logger.info(
            "transcribe session=%s lang=%s bytes=%s frames=%s dur=%.3fs out_chars=%s elapsed=%.3fs",
            session_id,
            lang or "auto",
            stats.get("bytes"),
            stats.get("frames"),
            stats.get("duration_sec"),
            len(text),
            elapsed,
        )

        # Log detected language for debugging
        if isinstance(transcription, dict):
            detected = transcription.get("language", "unknown")
            logger.info(f"Whisper detected language: {detected}")

        # Apply Light+ formatting (vocabulary fixes, punctuation, capitalization)
        text = apply_light_plus(text)

        return {"text": text}
    except Exception as e:
        logger.exception("Transcription failed (session=%s)", session_id)
        return JSONResponse(status_code=500, content={"error": str(e)})
    finally:
        # Clean up temp files
        for p in [temp_path, converted_path]:
            if p and os.path.exists(p):
                try:
                    os.remove(p)
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)


@app.post("/format")
async def format_endpoint(req: FormatRequest, request: Request):
    """Apply AI enhancement. Incoming text is already Light+ formatted."""

    text = req.text or ""
    if not text:
        return {"text": ""}

    settings = get_settings()
    if not req.assistive and not settings.ai_formatting_enabled:
        return {"text": text}

    session_id = _session_id_from_request(request)
    start_time = time.perf_counter()
    formatted = await apply_ai_formatting(text, assistive=req.assistive)
    elapsed = time.perf_counter() - start_time
    logger.info(
        "format session=%s provider=%s assistive=%s in_chars=%s out_chars=%s elapsed=%.3fs",
        session_id,
        settings.ai_provider,
        req.assistive,
        len(text),
        len(formatted or ""),
        elapsed,
    )
    return {"text": formatted}


@app.post("/stt_and_format")
async def stt_and_format(
    request: Request,
    audio: UploadFile = File(...),  # noqa: B008  # FastAPI pattern
    instruction: str | None = Form(None),
):
    t = await transcribe_endpoint(request=request, audio=audio)
    if isinstance(t, JSONResponse):
        return t
    txt = t.get("text", "")
    settings = get_settings()
    if not settings.ai_formatting_enabled:
        return {"text": txt}
    f = await format_endpoint(FormatRequest(text=txt, instruction=instruction), request=request)
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
    logger.info(
        "stream-ws session=%s lang=%s sample_rate=%s encoding=%s",
        session.session_id,
        session.language,
        session.sample_rate,
        session.encoding,
    )

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
        except asyncio.CancelledError as exc:
            logger.debug("SSE stream cancelled", exc_info=exc)
        finally:
            try:
                _subs.remove(q)
            except ValueError as exc:
                logger.debug("Subscriber missing during cleanup", exc_info=exc)

    return StreamingResponse(gen(), media_type="text/event-stream")


@app.get("/version")
async def version():
    """Return basic version and readiness info for integrations."""
    try:
        from importlib.metadata import version as _pkg_version  # type: ignore

        pkg_ver = _pkg_version("CodeScribe")
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


@app.get("/model")
async def get_model():
    """Get current Whisper model info."""
    return {
        "variant": _variant or "small",
        "path": str(WHISPER_DIR) if WHISPER_DIR else None,
        "loaded": whisper_model is not None,
    }


@app.post("/model/set")
async def set_model(variant: str = Body(..., embed=True)):  # noqa: B008  # FastAPI pattern
    """Switch Whisper model variant at runtime.

    Supported variants: small, medium, large-v3, large-v3-turbo
    """
    global whisper_model, WHISPER_DIR, _variant

    variant = (variant or "").strip().lower()
    valid_variants = {"small", "medium", "large-v3", "large-v3-turbo"}
    if variant not in valid_variants:
        return {"ok": False, "error": f"Invalid variant. Must be one of: {valid_variants}"}

    try:
        from . import stt as stt_mod

        # Try to find the model path
        path = stt_mod.find_variant_path(variant)
        if path is None:
            return {
                "ok": False,
                "error": f"Model '{variant}' not found. Download it first via get_models.py",
            }

        # Update global state
        whisper_model = None  # Force reload on next transcription
        WHISPER_DIR = path
        _variant = variant
        os.environ["WHISPER_DIR"] = path
        os.environ["WHISPER_VARIANT"] = variant

        logger.info(f"Whisper model switched to: {variant} at {path}")
        return {"ok": True, "variant": variant, "path": path}
    except Exception as e:
        logger.error(f"Failed to switch model: {e}")
        return {"ok": False, "error": str(e)}


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


def _strip_code_fences(text: str) -> str:
    cleaned = text.strip()
    if cleaned.startswith("```"):
        cleaned = cleaned.strip("`")
    cleaned = cleaned.replace("```json", "").replace("```", "").strip()
    return cleaned


def _parse_learning_entries(raw: str) -> list[dict[str, Any]]:
    """Strict parser for LLM responses -> list[{'term','mispronunciations'}]."""
    cleaned = _strip_code_fences(raw)
    data = json.loads(cleaned)
    if isinstance(data, dict):
        # Some models wrap list in {"entries": [...]}
        data = data.get("entries") or data.get("data") or data.get("items") or []
    if not isinstance(data, list):
        raise ValueError("LLM returned invalid format (expected list)")

    entries: list[dict[str, Any]] = []
    seen: set[tuple[str, tuple[str, ...]]] = set()

    for item in data:
        term = str(item.get("term") or "").strip()
        variants_raw = item.get("mispronunciations") or []
        if not term or not variants_raw:
            continue
        variants: list[str] = []
        for v in variants_raw:
            s = str(v or "").strip()
            if s:
                variants.append(s[:128])
        if not variants:
            continue
        term = term[:128]
        key = (term.lower(), tuple(m.lower() for m in variants))
        if key in seen:
            continue
        seen.add(key)
        entries.append({"term": term, "mispronunciations": variants})
    return entries


def _diff_learning_entries(reference: str, transcript: str) -> list[dict[str, Any]]:
    """Lightweight difflib fallback when AI is unavailable."""
    import difflib

    ref_tokens = reference.split()
    hyp_tokens = transcript.split()
    sm = difflib.SequenceMatcher(None, ref_tokens, hyp_tokens)
    entries: list[dict[str, Any]] = []
    seen: set[tuple[str, str]] = set()
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            continue
        ref_chunk = " ".join(ref_tokens[i1:i2]).strip()
        hyp_chunk = " ".join(hyp_tokens[j1:j2]).strip()
        if not ref_chunk or not hyp_chunk:
            continue
        key = (ref_chunk.lower(), hyp_chunk.lower())
        if key in seen:
            continue
        seen.add(key)
        entries.append({"term": ref_chunk, "mispronunciations": [hyp_chunk]})
    return entries


def _merge_entries(*entry_lists: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: list[dict[str, Any]] = []
    seen: set[tuple[str, tuple[str, ...]]] = set()
    for entries in entry_lists:
        for entry in entries or []:
            term = str(entry.get("term") or "").strip()
            variants = [
                str(v or "").strip()
                for v in entry.get("mispronunciations") or []
                if str(v or "").strip()
            ]
            if not term or not variants:
                continue
            key = (term.lower(), tuple(v.lower() for v in variants))
            if key in seen:
                continue
            seen.add(key)
            merged.append({"term": term, "mispronunciations": variants})
    return merged


def _parse_sentence_list(raw: str, *, limit: int | None = None) -> list[str]:
    cleaned = _strip_code_fences(raw)
    data = json.loads(cleaned)
    if isinstance(data, dict):
        data = data.get("sentences") or data.get("items") or data.get("data") or []
    if not isinstance(data, list):
        raise ValueError("Invalid sentence list")
    sentences: list[str] = []
    for item in data:
        s = str(item or "").strip()
        if not s:
            continue
        sentences.append(s)
        if limit and len(sentences) >= limit:
            break
    return sentences


def _fallback_sentences(topic: str, count: int = 5) -> list[str]:
    topic_slug = sanitize_topic(topic)
    return [
        f"Krótkie zdanie kalibracyjne #{i + 1} dotyczące tematu {topic_slug} — przeczytaj je głośno."
        for i in range(count)
    ]


class LearnRequest(BaseModel):
    topic: str
    reference: str | None = None
    transcript: str


@app.post("/lab/learn")
async def lab_learn(req: LearnRequest):
    """Active Learning endpoint: Diff reference vs transcript and update dictionary."""
    if not req.reference or not req.transcript:
        return {"ok": False, "error": "Missing text"}

    topic_slug = sanitize_topic(req.topic)
    settings = get_settings()
    ai_available = settings.ai_formatting_enabled and bool(
        os.environ.get("HARMONY_API_KEY")
        or os.environ.get("LIBRAXIS_API_KEY")
        or os.environ.get("OPENAI_API_KEY")
    )

    ai_entries: list[dict[str, Any]] = []
    ai_error: str | None = None

    if ai_available:
        prompt = (
            "Role: Build a lexicon of mis-heard phrases for a speech recognizer.\n"
            f"Topic: {topic_slug}\n"
            f"Reference (correct text): {req.reference}\n"
            f"Transcript (what was heard): {req.transcript}\n\n"
            "Ignore punctuation/casing differences. Focus on phonetic mistakes. "
            "Return ONLY JSON list: "
            "[{'term': 'CorrectTerm', 'mispronunciations': ['WrongTerm1','WrongTerm2']}]."
        )
        try:
            response_text = await run_chat_session(
                [{"role": "user", "content": prompt}], settings=settings
            )
            ai_entries = _parse_learning_entries(response_text)
        except Exception as e:  # pragma: no cover - depends on remote LLM
            ai_error = str(e)
            logger.warning("LLM learning failed (%s), will fallback to difflib", e)

    diff_entries = _diff_learning_entries(req.reference, req.transcript)
    merged_entries = _merge_entries(ai_entries, diff_entries)

    if not merged_entries:
        return {
            "ok": False,
            "error": ai_error or "No actionable differences found",
            "source": "diff",
        }

    count, target_file = append_lexicon_entries(topic_slug, merged_entries)
    reload_lexicon()
    total_entries = len(load_lexicon_entries(topic_slug))

    return {
        "ok": True,
        "learned": count,
        "file": str(target_file),
        "source": "ai" if ai_entries else "diff",
        "ai_error": ai_error,
        "total_entries": total_entries,
    }


@app.get("/lab/calibrate/generate")
async def lab_calibrate_generate(topic: str):
    """Generate calibration sentences for a topic."""
    settings = get_settings()
    topic_slug = sanitize_topic(topic)
    prompt = (
        f"Topic: {topic_slug}. Language: Polish.\n"
        f"Task: Generate 5 difficult sentences containing technical/domain vocabulary, "
        f"slang, or phonetic traps for this topic.\n"
        f"Return ONLY a JSON list of strings: ['sentence1', ...]"
    )

    try:
        response_text = await run_chat_session(
            [{"role": "user", "content": prompt}], settings=settings
        )
        sentences = _parse_sentence_list(response_text, limit=5)
        return {"sentences": sentences}
    except Exception as e:
        logger.error(f"Calibration generation failed: {e}")
        return {"sentences": _fallback_sentences(topic_slug, count=5), "error": str(e)}


@app.get("/lab/calibrate/wizard")
async def lab_calibrate_wizard(topic: str):
    """Return a batch of 10 calibration sentences for wizard flow."""
    settings = get_settings()
    topic_slug = sanitize_topic(topic)
    prompt = (
        f"Topic: {topic_slug}. Language: Polish.\n"
        f"Task: Generate 10 concise sentences that stress tricky phonetics for ASR. "
        f"Include domain-specific terms and hard-to-pronounce phrases.\n"
        f"Return ONLY JSON list of strings."
    )
    try:
        response_text = await run_chat_session(
            [{"role": "user", "content": prompt}], settings=settings
        )
        sentences = _parse_sentence_list(response_text, limit=10)
        if not sentences:
            raise ValueError("Empty sentence list")
        return {"sentences": sentences}
    except Exception as e:
        logger.error(f"Calibration wizard generation failed: {e}")
        return {"sentences": _fallback_sentences(topic_slug, count=10), "error": str(e)}


@app.get("/lab/lexicon")
async def lab_lexicon(topic: str | None = None, limit: int = 50):
    """Preview lexicon entries for a topic (or all)."""
    topic_slug = sanitize_topic(topic) if topic else None
    entries = load_lexicon_entries(topic_slug)
    count = len(entries)
    limited = entries[: limit if limit and limit > 0 else None]
    return {"topic": topic_slug or "all", "count": count, "entries": limited}


@app.get("/lab/lexicon/export")
async def lab_lexicon_export(topic: str | None = None):
    topic_slug = sanitize_topic(topic) if topic else None
    entries = load_lexicon_entries(topic_slug)
    return {"topic": topic_slug or "all", "count": len(entries), "entries": entries}


@app.post("/lab/lexicon/clear")
async def lab_lexicon_clear(topic: str):
    topic_slug = sanitize_topic(topic)
    path = lexicon_path_for_topic(topic_slug)
    cleared = False
    if path.exists():
        path.unlink()
        cleared = True
    reload_lexicon()
    return {"ok": True, "cleared": cleared, "topic": topic_slug}


@app.post("/lab/lexicon/refresh")
async def lab_lexicon_refresh():
    reload_lexicon()
    total = len(load_lexicon_entries())
    return {"ok": True, "count": total}


if __name__ == "__main__":
    from .codescribe_server import main as server_main

    sys.exit(server_main())
