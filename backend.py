#!/usr/bin/env python3
"""
backend.py — VistaScribe local backend (FastAPI)

Runs a lightweight HTTP server with preloaded models:
- MLX Whisper for speech-to-text (local, no API key)
- MLX-LM for optional text formatting (local, no API key)

Endpoints:
- GET  /healthz                  -> { ok: true }
- POST /transcribe  (multipart)  -> { text }
- POST /format      (json)       -> { text }
- POST /stt_and_format (multipart + optional 'instruction' form field) -> { text }
- POST /action      (json {action}) -> { ok, state }   [for UI widgets]
- GET  /events      (SSE)        -> stream of { state }

Env vars:
- WHISPER_DIR       : path to whisper model (default ./models/whisper-large-v3-turbo)
- WHISPER_VARIANT   : large-v3-turbo | medium (used when WHISPER_DIR not set)
- LLM_ID            : path or HF repo for MLX-LM (optional)
- FORMAT_ENABLED    : 1 (default) or 0
- HOST, PORT        : bind address (default 127.0.0.1:8237)
- TEMPERATURE, MAX_NEW_TOKENS, TOP_P, TOP_K

Notes:
- Uses path_utils.normalize_model_path to work around MLX path casing on macOS.
- The tray .app can continue to work as before; this backend exists for Quick Action (Q2)
  and for future web widgets via /events and /action.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from collections.abc import AsyncIterator

# MLX Whisper
import mlx_whisper as whisper
from fastapi import Body, FastAPI, File, Form, UploadFile
from fastapi.responses import JSONResponse, StreamingResponse
from mlx_whisper.load_models import load_model as load_whisper
from pydantic import BaseModel

# Optional audio decoding helper (API may vary by version)
try:  # mlx-audio >= 0.2.x sometimes does not expose `load`
    from mlx_audio import load as load_audio  # type: ignore
except Exception:  # pragma: no cover
    load_audio = None  # we'll fallback to WAV-only parsing

# MLX-LM
from mlx_lm import generate as lm_generate, load as load_lm
from mlx_lm.generate import make_sampler

from path_utils import normalize_model_path

logging.basicConfig(
    level=os.environ.get("LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger("vista-backend")

app = FastAPI(title="VistaScribe-backend")

# --- Config / Model load ---
REPO_ROOT = os.path.dirname(os.path.abspath(__file__))

# Whisper path selection (reuse logic from stt.py)
_variant = os.environ.get("WHISPER_VARIANT", "").strip().lower()
if not os.environ.get("WHISPER_DIR"):
    candidates: list[str] = []
    if _variant in {"large-v3-turbo", "medium"}:
        candidates.append(os.path.join(REPO_ROOT, "models", f"whisper-{_variant}"))
        candidates.append(os.path.join(REPO_ROOT, f"whisper-{_variant}"))
    else:
        for v in ("large-v3-turbo", "medium"):
            candidates.append(os.path.join(REPO_ROOT, "models", f"whisper-{v}"))
            candidates.append(os.path.join(REPO_ROOT, f"whisper-{v}"))
    _default_whisper_path = next(
        (c for c in candidates if os.path.isdir(c)),
        os.path.join(REPO_ROOT, "models", "whisper-large-v3-turbo"),
    )
else:
    _default_whisper_path = os.environ.get("WHISPER_DIR")

WHISPER_DIR = normalize_model_path(_default_whisper_path)

try:
    whisper_model = load_whisper(WHISPER_DIR)
    logger.info(f"MLX Whisper model loaded from: {WHISPER_DIR}")
except Exception as e:
    logger.error(f"Failed to load MLX Whisper model: {e}")
    whisper_model = None

# LLM (optional)
FORMAT_ENABLED = os.environ.get("FORMAT_ENABLED", "1").strip().lower() not in {
    "0",
    "false",
    "no",
    "off",
}
LLM_ID_RAW = os.environ.get("LLM_ID")
# Auto-detect default local LLM if not provided and formatting is enabled
if FORMAT_ENABLED and not LLM_ID_RAW:
    _default_llm = os.path.join(REPO_ROOT, "models", "bielik-4.5b-mxfp4-mlx")
    if os.path.isdir(_default_llm):
        LLM_ID_RAW = _default_llm

_model = None
_tok = None
_llm_id = None

TEMPERATURE = float(os.environ.get("TEMPERATURE", "0.2"))
TOP_P = float(os.environ.get("TOP_P", "0.0"))
TOP_K = int(os.environ.get("TOP_K", "0"))
MAX_NEW_TOKENS = int(os.environ.get("MAX_NEW_TOKENS", "128"))

SYSTEM_PROMPT = (
    "Sformatuj polski transkrypt: dodaj interpunkcję, popraw wielkie litery, "
    "nie zmieniaj sensu ani słów, nie dodawaj komentarza."
)

if FORMAT_ENABLED and LLM_ID_RAW:
    try:
        _llm_id = normalize_model_path(LLM_ID_RAW) or LLM_ID_RAW
        _model, _tok = load_lm(_llm_id)
        logger.info(f"MLX-LM model loaded: {_llm_id}")
    except Exception as e:
        logger.error(f"Failed to load MLX-LM model '{LLM_ID_RAW}': {e}")
        _model = _tok = None
else:
    logger.info("FORMAT_ENABLED=0 or LLM_ID not set; /format will passthrough.")


# --- State broadcasting (SSE) ---
_state: str = "idle"
_subs: list[asyncio.Queue[str]] = []


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
    return {"ok": ok, "state": _state}


class FormatRequest(BaseModel):
    text: str
    instruction: str | None = None


@app.post("/transcribe")
async def transcribe_endpoint(audio: UploadFile = File(...)):  # noqa: B008  # FastAPI pattern
    if whisper_model is None:
        return JSONResponse(status_code=500, content={"error": "Whisper not initialized"})
    try:
        audio_bytes = await audio.read()

        # Decode to (samples, sr)
        samples = None
        sr = None
        if load_audio is not None:
            samples, sr = load_audio(audio_bytes)  # handles decoding + resample
        else:
            # Fallback: accept only WAV bytes when mlx_audio.load is unavailable
            import io
            import wave

            import numpy as np  # lazy imports

            try:
                with wave.open(io.BytesIO(audio_bytes), "rb") as wf:
                    sr = wf.getframerate()
                    ch = wf.getnchannels()
                    sw = wf.getsampwidth()
                    n = wf.getnframes()
                    raw = wf.readframes(n)
                if sw == 2:
                    arr = np.frombuffer(raw, dtype=np.int16)
                    scale = 32768.0
                elif sw == 4:
                    arr = np.frombuffer(raw, dtype=np.int32)
                    scale = 2147483648.0
                else:
                    arr = np.frombuffer(raw, dtype=np.uint8).astype(np.int16)
                    scale = 128.0
                if ch and ch > 1:
                    arr = arr.reshape(-1, ch).mean(axis=1)
                samples = (arr.astype(np.float32) / scale).astype(np.float32)
            except Exception:
                msg = (
                    "Non-WAV audio upload requires mlx-audio >= 0.2 with load(); "
                    "please send WAV or install mlx-audio with load() API."
                )
                return JSONResponse(status_code=400, content={"error": msg})

        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            None, lambda: whisper.transcribe(whisper_model, samples, sr)
        )
        text = (result.get("text") or "").strip()
        return {"text": text}
    except Exception as e:
        logger.exception("Transcription failed")
        return JSONResponse(status_code=500, content={"error": str(e)})


@app.post("/format")
async def format_endpoint(req: FormatRequest):
    if not FORMAT_ENABLED:
        return {"text": req.text}
    if _model is None or _tok is None:
        return {"text": req.text}
    try:
        # Prefer chat template when available
        try:
            if hasattr(_tok, "apply_chat_template"):
                messages = [
                    {"role": "system", "content": req.instruction or SYSTEM_PROMPT},
                    {"role": "user", "content": req.text},
                ]
                prompt = _tok.apply_chat_template(messages, add_generation_prompt=True)
            else:
                raise AttributeError
        except Exception:
            prompt = f"System: {req.instruction or SYSTEM_PROMPT}\nUser: {req.text}\nAssistant:"

        sampler = make_sampler(temp=TEMPERATURE, top_p=TOP_P, top_k=TOP_K)
        loop = asyncio.get_event_loop()
        out = await loop.run_in_executor(
            None,
            lambda: lm_generate(_model, _tok, prompt, max_tokens=MAX_NEW_TOKENS, sampler=sampler),
        )
        return {"text": (out or "").strip()}
    except Exception as e:
        logger.exception("Formatting failed")
        return JSONResponse(status_code=500, content={"error": str(e)})


@app.post("/stt_and_format")
async def stt_and_format(
    audio: UploadFile = File(...),  # noqa: B008  # FastAPI pattern
    instruction: str | None = Form(None),
):
    t = await transcribe_endpoint(audio)
    if isinstance(t, JSONResponse):
        return t
    txt = t.get("text", "")
    if not FORMAT_ENABLED:
        return {"text": txt}
    f = await format_endpoint(FormatRequest(text=txt, instruction=instruction))
    return f


@app.post("/action")
async def action(payload: dict = Body(...)):  # noqa: B008  # FastAPI pattern
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
    q: asyncio.Queue[str] = asyncio.Queue()
    _subs.append(q)

    async def gen() -> AsyncIterator[str]:
        try:
            # initial
            yield f"data: {json.dumps({'state': _state})}\n\n"
            while True:
                data = await q.get()
                yield f"data: {data}\n\n"
        except asyncio.CancelledError:
            pass
        finally:
            try:
                _subs.remove(q)
            except ValueError:
                pass

    return StreamingResponse(gen(), media_type="text/event-stream")


if __name__ == "__main__":
    import uvicorn

    host = os.environ.get("HOST", "127.0.0.1")
    port = int(os.environ.get("PORT", "8237"))
    uvicorn.run("backend:app", host=host, port=port, reload=False)
