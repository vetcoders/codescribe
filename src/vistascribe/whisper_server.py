#!/usr/bin/env python3
"""
whisper_server.py — Separate FastAPI server for MLX Whisper.

Endpoints:
- GET  /healthz
- POST /transcribe  (multipart file: audio)

This module intentionally keeps the logic minimal for integration. If mlx_whisper
is unavailable, /healthz ok=False and /transcribe returns 500.
"""

from __future__ import annotations

import contextlib
import logging
import os

from fastapi import FastAPI, File, HTTPException, UploadFile
from fastapi.responses import JSONResponse

try:
    import mlx_whisper as whisper  # type: ignore
    from mlx_whisper.load_models import load_model as load_whisper  # type: ignore
except Exception:  # pragma: no cover
    whisper = None  # type: ignore
    load_whisper = None  # type: ignore

from .path_utils import normalize_model_path, repo_root

logger = logging.getLogger("whisper-server")


def _configure_logging() -> None:
    if logging.getLogger().handlers:
        return
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO").upper(),
        format="%(asctime)s - %(levelname)s - %(message)s",
    )


app = FastAPI(title="VistaScribe-whisper")

REPO_ROOT = str(repo_root())
_whisper_dir = os.environ.get("WHISPER_DIR") or os.path.join(
    REPO_ROOT, "models", "whisper-large-v3-turbo"
)
WHISPER_DIR = normalize_model_path(_whisper_dir)

MAX_UPLOAD_MB = int(os.environ.get("WHISPER_MAX_UPLOAD_MB", "20"))
MAX_UPLOAD_BYTES = MAX_UPLOAD_MB * 1024 * 1024
_CHUNK_SIZE = 1024 * 1024  # 1 MiB
_ALLOWED_EXTS = {".wav", ".mp3", ".m4a", ".flac", ".ogg", ".webm"}
_ALLOWED_MIME = {
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

_whisper_model: object | None = None
if whisper is not None and load_whisper is not None:
    try:
        _whisper_model = load_whisper(WHISPER_DIR)
        logger.info(f"Whisper loaded: {WHISPER_DIR}")
    except Exception as e:  # pragma: no cover (depends on local env)
        logger.error(f"Failed to load Whisper: {e}")
        _whisper_model = None


@app.get("/healthz")
async def healthz():
    return {"ok": _whisper_model is not None}


@app.post("/transcribe")
async def transcribe(audio: UploadFile = File(...)):  # noqa: B008
    if _whisper_model is None or whisper is None:
        return JSONResponse(status_code=500, content={"error": "Whisper not initialized"})
    filename = (audio.filename or "audio.wav").strip()
    ext = os.path.splitext(filename)[1].lower() or ".wav"
    if ext not in _ALLOWED_EXTS:
        raise HTTPException(status_code=415, detail="Unsupported audio type")
    if audio.content_type and audio.content_type.lower() not in _ALLOWED_MIME:
        raise HTTPException(status_code=415, detail="Unsupported audio MIME type")
    header_len = audio.headers.get("content-length") if audio.headers else None
    if header_len:
        with contextlib.suppress(ValueError):
            if int(header_len) > MAX_UPLOAD_BYTES:
                raise HTTPException(status_code=413, detail="Audio file too large")
    path = None
    try:
        # Let mlx_whisper transcribe from file path when possible.
        # For simplicity in this stub, save to a temp file and ensure cleanup.
        import tempfile

        total = 0
        with tempfile.NamedTemporaryFile(delete=False, suffix=ext or ".wav") as tmp:
            while True:
                chunk = await audio.read(_CHUNK_SIZE)
                if not chunk:
                    break
                total += len(chunk)
                if total > MAX_UPLOAD_BYTES:
                    raise HTTPException(status_code=413, detail="Audio file too large")
                tmp.write(chunk)
            path = tmp.name

        res = whisper.transcribe(path)
        if not res or not isinstance(res, dict) or ("text" not in res):
            raise ValueError("Whisper transcription returned empty result")
        text = (res.get("text") or "").strip()
        return {"text": text}
    except Exception as e:
        logger.exception("Transcription failed")
        return JSONResponse(status_code=500, content={"error": str(e)})
    finally:
        if path and os.path.exists(path):
            try:
                os.remove(path)
            except Exception:
                # best-effort cleanup
                pass


if __name__ == "__main__":
    import uvicorn

    host = os.environ.get("HOST", "127.0.0.1")
    port = int(os.environ.get("PORT", "8238"))
    _configure_logging()
    uvicorn.run("vistascribe.whisper_server:app", host=host, port=port, reload=False)
