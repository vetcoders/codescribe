#!/usr/bin/env python3
"""
lm_server.py — Separate FastAPI server for MLX-LM formatting.

Endpoints:
- GET  /healthz
- POST /format {text}

This is a minimal adapter around mlx_lm; for CI/tests we keep behavior simple
and echo text when model isn't loaded.
"""
from __future__ import annotations

import logging
import os

from fastapi import FastAPI
from pydantic import BaseModel

try:
    from mlx_lm import generate as lm_generate, load as load_lm  # type: ignore
    from mlx_lm.generate import make_sampler  # type: ignore
except Exception:  # pragma: no cover
    load_lm = None  # type: ignore
    lm_generate = None  # type: ignore
    make_sampler = None  # type: ignore

from path_utils import normalize_model_path

logging.basicConfig(level=os.environ.get("LOG_LEVEL", "INFO").upper(),
                    format="%(asctime)s - %(levelname)s - %(message)s")
logger = logging.getLogger("lm-server")

app = FastAPI(title="VistaScribe-llm")

REPO_ROOT = os.path.dirname(os.path.abspath(__file__))
LLM_ID = normalize_model_path(os.environ.get("LLM_ID", "").strip())
TEMPERATURE = float(os.environ.get("TEMPERATURE", "0.2"))
TOP_P = float(os.environ.get("TOP_P", "0.0"))
TOP_K = int(os.environ.get("TOP_K", "0"))
MAX_NEW_TOKENS = int(os.environ.get("MAX_NEW_TOKENS", "128"))

_model = None
_tok = None

if load_lm is not None and LLM_ID:
    try:
        _model, _tok = load_lm(LLM_ID)
        logger.info(f"MLX-LM loaded: {LLM_ID}")
    except Exception as e:  # pragma: no cover
        logger.error(f"Failed to load LLM: {e}")
        _model = _tok = None


class FormatRequest(BaseModel):
    text: str


@app.get("/healthz")
async def healthz():
    return {"ok": _model is not None and _tok is not None}


@app.post("/format")
async def format_endpoint(req: FormatRequest):
    # If model not ready or mlx_lm unavailable, echo input to keep pipeline working
    if _model is None or _tok is None or lm_generate is None or make_sampler is None:
        return {"text": req.text}
    try:
        sampler = make_sampler(temp=TEMPERATURE, top_p=TOP_P, top_k=TOP_K)
        out = lm_generate(_model, _tok, req.text, max_tokens=MAX_NEW_TOKENS, sampler=sampler)
        return {"text": (out or "").strip()}
    except Exception:
        logger.exception("Formatting failed")
        return {"text": req.text}


if __name__ == "__main__":
    import uvicorn
    host = os.environ.get("HOST", "127.0.0.1")
    port = int(os.environ.get("PORT", "8239"))
    uvicorn.run("lm_server:app", host=host, port=port, reload=False)
