#!/usr/bin/env python3
"""
whisper_server.py — Separate FastAPI server for MLX Whisper.

Endpoints:
- GET  /healthz
- POST /transcribe  (multipart file: audio, optional: language)

This module intentionally keeps the logic minimal for integration. If mlx_whisper
is unavailable, /healthz ok=False and /transcribe returns 500.

Supports:
- Language specification (pl, en, auto)
- Custom vocabulary via initial_prompt from JSONL dictionaries
"""

from __future__ import annotations

import contextlib
import json
import logging
import os

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.responses import JSONResponse

try:
    import mlx_whisper as whisper  # type: ignore
    from mlx_whisper.load_models import load_model as load_whisper  # type: ignore
except Exception:  # pragma: no cover
    whisper = None  # type: ignore
    load_whisper = None  # type: ignore

from .formatting import apply_light_plus
from .path_utils import normalize_model_path, repo_root

logger = logging.getLogger("whisper-server")


def _configure_logging() -> None:
    if logging.getLogger().handlers:
        return
    logging.basicConfig(
        level=os.environ.get("LOG_LEVEL", "INFO").upper(),
        format="%(asctime)s - %(levelname)s - %(message)s",
    )


app = FastAPI(title="CodeScribe-whisper")

REPO_ROOT = str(repo_root())

# Whisper model selection: prefer WHISPER_DIR if set, otherwise use WHISPER_VARIANT
# Default to "small" for faster startup and lower memory usage
_variant = os.environ.get("WHISPER_VARIANT", "small").strip().lower()
if os.environ.get("WHISPER_DIR"):
    _whisper_dir = os.environ["WHISPER_DIR"]
else:
    # Search for model in order of preference
    _candidates = [
        os.path.join(REPO_ROOT, "models", f"whisper-{_variant}"),
    ]
    # Fallback candidates if preferred variant not found
    for v in ("small", "medium", "large-v3-turbo", "large-v3"):
        if v != _variant:
            _candidates.append(os.path.join(REPO_ROOT, "models", f"whisper-{v}"))
    _whisper_dir = next(
        (c for c in _candidates if os.path.isdir(c)),
        os.path.join(REPO_ROOT, "models", "whisper-small"),  # Final fallback
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


def _load_dictionary_terms(dict_path: str, max_terms: int = 100) -> list[str]:
    """Load terms from JSONL dictionary file."""
    terms: list[str] = []
    if not os.path.exists(dict_path):
        return terms
    try:
        with open(dict_path, encoding="utf-8") as f:
            for line in f:
                if len(terms) >= max_terms:
                    break
                try:
                    entry = json.loads(line.strip())
                    if "term" in entry:
                        terms.append(entry["term"])
                except json.JSONDecodeError:
                    continue
    except Exception as e:
        logger.warning(f"Failed to load dictionary {dict_path}: {e}")
    return terms


def _build_initial_prompt() -> str:
    """Build initial_prompt from available dictionaries for better transcription accuracy."""
    assets_dir = os.path.join(REPO_ROOT, "assets")

    # Try multiple dictionary locations
    dict_paths = [
        os.path.join(assets_dir, "veterinary.jsonl"),
        os.path.join(assets_dir, "programming.jsonl"),
        os.path.join(REPO_ROOT, "src", "codescribe", "assets", "veterinary.jsonl"),
        os.path.join(REPO_ROOT, "src", "codescribe", "assets", "programming.jsonl"),
    ]

    all_terms = []
    for path in dict_paths:
        terms = _load_dictionary_terms(path, max_terms=50)
        if terms:
            logger.info(f"Loaded {len(terms)} terms from {os.path.basename(path)}")
            all_terms.extend(terms)

    if not all_terms:
        # Fallback: common Polish veterinary/programming terms
        all_terms = [
            "pacjent",
            "diagnoza",
            "leczenie",
            "badanie",
            "temperatura",
            "API",
            "endpoint",
            "serwer",
            "backend",
            "frontend",
            "transcribe",
            "transkrypcja",
            "Whisper",
            "model",
        ]

    # Create prompt with terms (Whisper uses this as context)
    return ", ".join(all_terms[:100])


# Pre-load initial prompt at startup
_initial_prompt: str = _build_initial_prompt()
if _initial_prompt:
    logger.info(f"Initial prompt loaded with {len(_initial_prompt.split(', '))} terms")


@app.get("/healthz")
async def healthz():
    return {"ok": _whisper_model is not None}


@app.post("/model/set")
async def set_model(body: dict):
    """Switch Whisper model variant at runtime.

    Args:
        body: {"variant": "small" | "medium" | "large-v3" | "large-v3-turbo"}

    Returns:
        {"ok": true, "variant": "...", "path": "..."} on success
        {"ok": false, "error": "..."} on failure
    """
    global _whisper_model, WHISPER_DIR

    if load_whisper is None:
        return JSONResponse(
            status_code=500,
            content={"ok": False, "error": "mlx_whisper not available"},
        )

    variant = body.get("variant", "").strip().lower()
    if not variant:
        return JSONResponse(
            status_code=400,
            content={"ok": False, "error": "Missing 'variant' field"},
        )

    # Build model path
    model_path = os.path.join(REPO_ROOT, "models", f"whisper-{variant}")

    if not os.path.isdir(model_path):
        return JSONResponse(
            status_code=404,
            content={
                "ok": False,
                "error": f"Model not found: whisper-{variant}",
            },
        )

    try:
        logger.info(f"Loading Whisper model: {model_path}")
        normalized_path = normalize_model_path(model_path)
        new_model = load_whisper(normalized_path)
        _whisper_model = new_model
        WHISPER_DIR = normalized_path
        logger.info(f"Whisper model switched to: {variant} at {normalized_path}")
        return {"ok": True, "variant": variant, "path": normalized_path}
    except Exception as e:
        logger.exception(f"Failed to load model {variant}")
        return JSONResponse(
            status_code=500,
            content={"ok": False, "error": str(e)},
        )


@app.post("/transcribe")
async def transcribe(
    audio: UploadFile = File(...),  # noqa: B008
    language: str | None = Form(None),
):
    """Transcribe audio file with optional language specification.

    Args:
        audio: Audio file (WAV, MP3, M4A, etc.)
        language: Language code (pl, en, auto, or None for auto-detection)
    """
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

        # Build transcription kwargs
        transcribe_kwargs = {
            "path_or_hf_repo": WHISPER_DIR,
            # Anti-repetition: prevent context bleeding between segments
            "condition_on_previous_text": False,
            # Note: logprob_threshold and no_speech_threshold were removed
            # because they were too aggressive and cut off valid transcriptions.
            # The defaults work well for most cases.
        }

        # Add language if specified (None or "auto" means auto-detect)
        if language and language.lower() not in ("auto", "none", ""):
            transcribe_kwargs["language"] = language.lower()
            logger.info(f"Using language: {language}")

        # Add initial_prompt for better accuracy with domain-specific terms
        if _initial_prompt:
            transcribe_kwargs["initial_prompt"] = _initial_prompt

        logger.info(f"Transcribing {filename} with kwargs: {list(transcribe_kwargs.keys())}")
        res = whisper.transcribe(path, **transcribe_kwargs)

        if not res or not isinstance(res, dict) or ("text" not in res):
            raise ValueError("Whisper transcription returned empty result")
        text = (res.get("text") or "").strip()
        # Apply Light+ formatting (vocabulary fixes, punctuation, capitalization)
        text = apply_light_plus(text)
        logger.info(f"Transcription result: {len(text)} chars")
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
    uvicorn.run("codescribe.whisper_server:app", host=host, port=port, reload=False)
