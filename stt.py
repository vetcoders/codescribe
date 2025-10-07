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
# If running from an app bundle, bundled models may live in:
#   <App>.app/Contents/Resources/Models
_bundled_models_dir = os.path.normpath(os.path.join(_repo_root, "..", "..", "Resources", "Models"))
# Allow choosing between large-v3-turbo and medium via env; fallback to what's present
_variant = os.environ.get("WHISPER_VARIANT", "").strip().lower()

# If a remote server URL is configured, skip local model initialization
whisper_model = None
WHISPER_DIR = None
if not WHISPER_SERVER_URL:
    # If WHISPER_DIR provided, use it directly (normalized) below; otherwise compute default
    if not os.environ.get("WHISPER_DIR"):
        candidates = []
        # If bundled small exists, prefer it for offline-first experience
        for name in ("whisper-small", "whisper-small-mlx"):
            candidates.append(os.path.join(_bundled_models_dir, name))
        # If explicit variant set, search repo models for it
        if _variant in {"small", "small-mlx", "large-v3-turbo", "large-v3", "medium"}:
            vmap = {"small": "small-mlx"}  # resolve alias
            vv = vmap.get(_variant, _variant)
            candidates.append(os.path.join(_repo_root, "models", f"whisper-{vv}"))
            candidates.append(os.path.join(_repo_root, f"whisper-{vv}"))
        else:
            # Default order: large-v3, large-v3-turbo, medium, then small
            for v in ("large-v3", "large-v3-turbo", "medium", "small-mlx", "small"):
                candidates.append(os.path.join(_repo_root, "models", f"whisper-{v}"))
                candidates.append(os.path.join(_repo_root, f"whisper-{v}"))
        # pick first existing, else fallback to bundled small, else turbo path in repo
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


def get_current_variant() -> str:
    """Best-effort name of the currently loaded local model variant.

    Returns one of: "large-v3-turbo", "large-v3", "medium", or "remote"/"unknown".
    """
    if WHISPER_SERVER_URL:
        return "remote"
    path = (WHISPER_DIR or "").lower()
    if "whisper-large-v3-turbo" in path:
        return "large-v3-turbo"
    if "whisper-large-v3" in path and "-turbo" not in path:
        return "large-v3"
    if "whisper-medium" in path:
        return "medium"
    if "whisper-small" in path:
        return "small"
    return "unknown"


def set_variant(variant: str) -> bool:
    """Switch the local MLX Whisper model at runtime.

    Loads the given `variant` ("medium", "large-v3", or "large-v3-turbo") from
    the repository's models directory. Returns True on success.
    """
    global whisper_model, WHISPER_DIR, _variant
    if WHISPER_SERVER_URL:
        logging.error("Cannot switch local model while WHISPER_SERVER_URL is set.")
        return False
    variant = (variant or "").strip().lower()
    if variant not in {"small", "small-mlx", "medium", "large-v3", "large-v3-turbo"}:
        logging.error(f"Unsupported variant: {variant}")
        return False
    # Resolve alias and search both repo models and bundled models
    vnorm = {"small": "small-mlx"}.get(variant, variant)
    base_candidates = [
        os.path.join(_repo_root, "models", f"whisper-{vnorm}"),
        os.path.join(_repo_root, f"whisper-{vnorm}"),
        os.path.join(_bundled_models_dir, f"whisper-{vnorm}"),
        os.path.join(_bundled_models_dir, vnorm),
        os.path.join(_bundled_models_dir, "whisper-small"),
    ]
    base = next((p for p in base_candidates if os.path.isdir(p)), None)
    if base is None:
        logging.error(
            f"Model directory not found for variant '{variant}'. Download via menu first."
        )
        return False
    if not os.path.isdir(base):
        logging.error(
            f"Model directory not found for variant '{variant}': {base}. Download via menu first."
        )
        return False
    new_dir = normalize_model_path(base)
    try:
        if load_model is None:
            raise RuntimeError("mlx_whisper not available")
        model = load_model(new_dir)
        whisper_model = model
        WHISPER_DIR = new_dir
        _variant = variant
        os.environ["WHISPER_DIR"] = new_dir
        os.environ["WHISPER_VARIANT"] = variant
        logging.info(f"Switched Whisper model to '{variant}' at: {new_dir}")
        return True
    except Exception as e:
        logging.error(f"Failed to switch model to '{variant}': {e}")
        return False


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
