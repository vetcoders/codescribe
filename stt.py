# stt.py (speech-to-text)
#
# Local transcription using MLX Whisper (no API key required).
# Loads the Whisper model once and transcribes audio files produced by audio.py.
#
# Key env vars:
# - WHISPER_DIR: absolute or relative path to whisper model directory.
#                If omitted, defaults to './models/whisper-large-v3-turbo' if present,
#                otherwise './whisper-large-v3-turbo' in repo root.
#                NOTE: mlx_whisper may be sensitive to uppercase in absolute
#                paths on macOS; we normalize '/Users' → '/users'.

import asyncio
import logging
import os

from dotenv import load_dotenv

# MLX Whisper (local fallback)
from path_utils import normalize_model_path

try:
    import mlx_whisper as whisper  # type: ignore
    from mlx_whisper.load_models import load_model  # type: ignore
except Exception:  # pragma: no cover
    whisper = None  # type: ignore
    load_model = None  # type: ignore

# --- setup ---
load_dotenv()
logging.basicConfig(
    level=os.environ.get("LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s - %(levelname)s - %(message)s",
)

# Optional remote server
WHISPER_SERVER_URL = os.environ.get("WHISPER_SERVER_URL", "").strip()

# --- model load (once) ---
_repo_root = os.path.dirname(os.path.abspath(__file__))
# Allow choosing between large-v3-turbo and medium via env; fallback to what's present
_variant = os.environ.get("WHISPER_VARIANT", "").strip().lower()

# If a remote server URL is configured, skip local model initialization
whisper_model = None
WHISPER_DIR = None
if not WHISPER_SERVER_URL:
    # If WHISPER_DIR provided, use it directly (normalized) below; otherwise compute default
    if not os.environ.get("WHISPER_DIR"):
        candidates = []
        if _variant in {"large-v3-turbo", "medium"}:
            candidates.append(os.path.join(_repo_root, "models", f"whisper-{_variant}"))
            candidates.append(os.path.join(_repo_root, f"whisper-{_variant}"))
        else:
            # No explicit variant: prefer large-v3-turbo if present, else medium
            for v in ("large-v3-turbo", "medium"):
                candidates.append(os.path.join(_repo_root, "models", f"whisper-{v}"))
                candidates.append(os.path.join(_repo_root, f"whisper-{v}"))
        # pick first existing, else default to models/whisper-large-v3-turbo path
        _default_whisper_path = next(
            (c for c in candidates if os.path.isdir(c)),
            os.path.join(_repo_root, "models", "whisper-large-v3-turbo"),
        )
    else:
        _default_whisper_path = os.environ.get("WHISPER_DIR")

    WHISPER_DIR = normalize_model_path(_default_whisper_path)

    try:
        if load_model is None:
            raise RuntimeError("mlx_whisper not available")
        whisper_model = load_model(WHISPER_DIR)
        logging.info(f"MLX Whisper model loaded from: {WHISPER_DIR}")
    except Exception as e:  # pragma: no cover (depends on local setup)
        logging.error(f"Failed to load MLX Whisper model at '{WHISPER_DIR}': {e}")
        whisper_model = None

# Language preference (None = auto). Read from env on startup if provided.
LANGUAGE_CODE = (
    os.environ.get("WHISPER_LANGUAGE") or os.environ.get("LANGUAGE") or ""
).strip().lower() or None


def set_language(code: str | None):
    """Set preferred language code ('pl', 'en', or None for auto)."""
    global LANGUAGE_CODE
    if code:
        code = code.strip().lower()
        if code not in ("pl", "en"):
            logging.warning(f"Unsupported language code '{code}', falling back to auto")
            code = None
    LANGUAGE_CODE = code
    logging.info(f"Whisper language set to: {LANGUAGE_CODE or 'auto'}")


def get_language() -> str | None:
    """Get current preferred language code or None for auto."""
    return LANGUAGE_CODE


def _http_post(url: str, files: dict):
    import requests  # local import to keep optional

    resp = requests.post(url, files=files, timeout=60)
    resp.raise_for_status()
    return resp.json()


async def transcribe(path: str) -> str | None:
    """Transcribe the audio file at the given path.

    If WHISPER_SERVER_URL is set, send the audio to a remote FastAPI server.
    Otherwise, use local MLX Whisper if available.
    """
    # Remote path
    if WHISPER_SERVER_URL:
        if not os.path.exists(path):
            logging.error(f"Audio file not found at path: {path}")
            return None
        url = WHISPER_SERVER_URL.rstrip("/") + "/transcribe"
        try:
            # Run in a worker thread and provision a loop for patched helpers
            def _call_in_thread():
                try:
                    loop = asyncio.new_event_loop()
                    asyncio.set_event_loop(loop)
                except Exception:
                    loop = None
                try:
                    with open(path, "rb") as f:
                        return _http_post(
                            url,
                            files={
                                "audio": (
                                    os.path.basename(path),
                                    f,
                                    "audio/wav",
                                )
                            },
                        )
                finally:
                    if loop is not None:
                        try:
                            loop.close()
                        except Exception:
                            pass

            loop = asyncio.get_event_loop()
            data = await loop.run_in_executor(None, _call_in_thread)
            return (data.get("text") or "").strip()
        except Exception as e:
            logging.error(f"Remote transcription error: {e}")
            return None

    # Local path
    if whisper_model is None or whisper is None:
        logging.error("Whisper not initialized. Set WHISPER_DIR or configure WHISPER_SERVER_URL.")
        return None
    if not os.path.exists(path):
        logging.error(f"Audio file not found at path: {path}")
        return None

    logging.info(f"Starting transcription for audio file: {path}")
    try:
        # Run in thread pool to avoid blocking heavy work
        loop = asyncio.get_event_loop()
        lang = LANGUAGE_CODE

        def _transcribe():
            return whisper.transcribe(
                path,
                path_or_hf_repo=WHISPER_DIR,
                verbose=False,
                language=lang,
                condition_on_previous_text=False,
            )

        result = await loop.run_in_executor(None, _transcribe)
        text = (result.get("text") or "").strip()
        logging.info(f"Transcription successful. Length: {len(text)} chars.")
        return text
    except Exception as e:
        logging.error(f"Error during local transcription: {e}", exc_info=True)
        return None
