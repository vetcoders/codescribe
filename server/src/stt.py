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
from pathlib import Path

from dotenv import load_dotenv

# MLX Whisper (local fallback)
from .path_utils import normalize_model_path, repo_root, user_data_root

logger = logging.getLogger(__name__)

# To keep the tray process lightweight, avoid importing heavy MLX modules
# at import time. We'll import them lazily on first local transcription or
# explicit model switch.
whisper = None  # type: ignore
load_model = None  # type: ignore

# --- setup ---
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

logging.basicConfig(
    level=os.environ.get("LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s - %(levelname)s - %(message)s",
)

# Optional remote server
WHISPER_SERVER_URL = os.environ.get("WHISPER_SERVER_URL", "").strip()

# --- model discovery (no heavy imports) ---
_repo_root = str(repo_root())
# If running from an app bundle, bundled models may live alongside Resources/
_bundled_models_dir = str(Path(_repo_root).parent / "Resources" / "Models")
# Allow choosing between large-v3-turbo and medium via env; fallback to what's present
_variant = os.environ.get("WHISPER_VARIANT", "").strip().lower()

# We intentionally do NOT import or load MLX here. Only compute a default
# directory if we are using local mode.
whisper_model = None
WHISPER_DIR = None
if not WHISPER_SERVER_URL:
    # If WHISPER_DIR provided, use it directly (normalized) below; otherwise compute default
    if not os.environ.get("WHISPER_DIR"):
        candidates = []
        # If bundled small exists, prefer it for offline-first experience
        for name in ("whisper-small", "whisper-small-mlx", "whisper-small-pl"):
            candidates.append(os.path.join(_bundled_models_dir, name))
        # If explicit variant set, search repo models for it
        if _variant in {
            "small",
            "small-mlx",
            "small-pl",
            "large-v3-turbo",
            "large-v3",
            "medium",
            "medium-pl",
        }:
            vmap = {"small": "small-mlx"}  # resolve alias
            vv = vmap.get(_variant, _variant)
            candidates.append(os.path.join(_repo_root, "models", f"whisper-{vv}"))
            candidates.append(os.path.join(_repo_root, f"whisper-{vv}"))
        else:
            # Default order: prefer medium/small PL if present, then others
            for v in (
                "medium-pl",
                "small-pl",
                "large-v3",
                "large-v3-turbo",
                "medium",
                "small-mlx",
                "small",
            ):
                candidates.append(os.path.join(_repo_root, "models", f"whisper-{v}"))
                candidates.append(os.path.join(_repo_root, f"whisper-{v}"))
        # pick first existing, else fallback to turbo path in repo
        _default_whisper_path: str | None = next(
            (c for c in candidates if os.path.isdir(c)),
            os.path.join(_repo_root, "models", "whisper-large-v3-turbo"),
        )
    else:
        _default_whisper_path = os.environ.get("WHISPER_DIR")

    WHISPER_DIR = normalize_model_path(_default_whisper_path)


def _ensure_local_model_loaded() -> bool:
    """Import MLX Whisper and load the local model on-demand.

    Returns True if ready for local transcription, False otherwise.
    """
    global whisper, load_model, whisper_model
    if WHISPER_SERVER_URL:
        return False
    if whisper_model is not None:
        return True
    try:
        if whisper is None or load_model is None:  # import lazily
            import importlib

            whisper = importlib.import_module("mlx_whisper")  # type: ignore
            load_model = importlib.import_module("mlx_whisper.load_models").load_model  # type: ignore
        if not WHISPER_DIR:
            return False
        whisper_model = load_model(WHISPER_DIR)  # type: ignore
        logging.info(f"MLX Whisper model loaded from: {WHISPER_DIR}")
        return True
    except Exception as e:  # pragma: no cover (depends on local setup)
        logging.error(f"Failed to initialize MLX Whisper: {e}")
        whisper_model = None
        return False


# Language preference (None = auto). Read from env on startup if provided.
_raw_lang = (os.environ.get("WHISPER_LANGUAGE") or os.environ.get("LANGUAGE") or "").strip().lower()
# "auto" is a UI label only - MLX Whisper expects None for auto-detect
LANGUAGE_CODE = None if _raw_lang in ("auto", "") else _raw_lang

# Serialize local MLX transcriptions to avoid concurrent Metal command buffers
_LOCAL_STT_LOCK: asyncio.Lock | None = None


def set_language(code: str | None):
    """Set preferred language code ('pl', 'en', 'auto', or None for auto)."""
    global LANGUAGE_CODE
    if code:
        code = code.strip().lower()
        if code in ("auto", ""):
            code = None  # auto-detect
        elif code not in ("pl", "en"):
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


def find_variant_path(variant: str) -> str | None:
    """Return normalized path for a given Whisper variant if present on disk."""

    if WHISPER_SERVER_URL:
        return None
    variant = (variant or "").strip().lower()
    if variant not in {"small", "small-mlx", "medium", "large-v3", "large-v3-turbo"}:
        return None
    vnorm = {"small": "small-mlx"}.get(variant, variant)
    candidates = [
        os.path.join(_repo_root, "models", f"whisper-{vnorm}"),
        os.path.join(_repo_root, f"whisper-{vnorm}"),
        os.path.join(_bundled_models_dir, f"whisper-{vnorm}"),
        os.path.join(_bundled_models_dir, vnorm),
        os.path.join(_bundled_models_dir, "whisper-small"),
    ]
    base = next((p for p in candidates if os.path.isdir(p)), None)
    if not base:
        return None
    return normalize_model_path(base)


def set_variant(variant: str) -> bool:
    """Switch the local MLX Whisper model at runtime without eager import."""

    global whisper_model, WHISPER_DIR, _variant
    if WHISPER_SERVER_URL:
        logging.error("Cannot switch local model while WHISPER_SERVER_URL is set.")
        return False
    path = find_variant_path(variant)
    if path is None:
        logging.error(
            f"Model directory not found for variant '{variant}'. Download via menu first."
        )
        return False

    whisper_model = None
    WHISPER_DIR = path
    _variant = (variant or "").strip().lower()
    os.environ["WHISPER_DIR"] = path
    os.environ["WHISPER_VARIANT"] = _variant
    logging.info(f"Prepared Whisper model '{_variant}' at: {path} (will load on demand)")
    return True


def _http_post(url: str, files: dict):
    import requests  # local import to keep optional

    resp = requests.post(url, files=files, timeout=60)
    resp.raise_for_status()
    return resp.json()


async def transcribe(
    input_data: str | bytes | bytearray, mime: str | None = None, lang: str | None = None
) -> dict:
    """Unified STT entry point.

    Accepts a file path (str) or raw bytes/bytearray. Returns a structured
    result dict:
      {"ok": True,  "text": "..."}
      {"ok": False, "error": "message", "code": "..."}

    - If WHISPER_SERVER_URL is set, sends to remote /transcribe.
    - Otherwise, uses local MLX Whisper with a single preloaded model and
      a global asyncio.Lock to serialize Metal work.
    """
    mime = (mime or "audio/wav").strip().lower()
    lang = (lang or LANGUAGE_CODE) or None
    if lang == "auto":
        lang = None  # MLX Whisper expects None for auto-detect, not "auto" string

    # Prepare a file path. If bytes provided, persist to a temp WAV/bytes file.
    temp_path = None
    if isinstance(input_data, bytes | bytearray):
        try:
            import tempfile

            suffix = ".wav" if "wav" in mime else ".bin"
            with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tf:
                tf.write(input_data)
                temp_path = tf.name
            path = temp_path
        except Exception as e:
            logging.error(f"Failed to persist input bytes: {e}")
            return {"ok": False, "error": str(e), "code": "persist-bytes"}
    else:
        path = str(input_data)

    # Remote mode
    if WHISPER_SERVER_URL:
        if not os.path.exists(path):
            if temp_path is None:  # only log missing file when not from bytes
                logging.error(f"Audio file not found at path: {path}")
            return {"ok": False, "error": "file-not-found", "code": "input"}
        url = WHISPER_SERVER_URL.rstrip("/") + "/transcribe"
        try:
            # Run in a worker thread (avoid blocking loop)
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
                                    mime or "audio/wav",
                                )
                            },
                        )
                finally:
                    if loop is not None:
                        try:
                            loop.close()
                        except Exception as exc:
                            logger.debug("Suppressed exception", exc_info=exc)

            data = await asyncio.get_event_loop().run_in_executor(None, _call_in_thread)
            text = (data.get("text") or "").strip()
            return {"ok": True, "text": text}
        except Exception as e:
            logging.error(f"Remote transcription error: {e}")
            return {"ok": False, "error": str(e), "code": "remote"}
        finally:
            if temp_path and os.path.exists(temp_path):
                try:
                    os.remove(temp_path)
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)

    # Local mode
    if not os.path.exists(path):
        if temp_path is None:
            logging.error(f"Audio file not found at path: {path}")
        return {"ok": False, "error": "file-not-found", "code": "input"}

    logging.info(f"Starting transcription for audio file: {path}")
    try:
        # Run in thread pool to avoid blocking heavy work, but serialize MLX work
        async def _get_lock() -> asyncio.Lock:
            global _LOCAL_STT_LOCK
            if _LOCAL_STT_LOCK is None:
                _LOCAL_STT_LOCK = asyncio.Lock()
            return _LOCAL_STT_LOCK

        lock = await _get_lock()
        async with lock:

            def _do_transcribe():
                # Simple path-based API (MLX Whisper handles decode internally)
                # This is more reliable than manual decode + samples
                if not _ensure_local_model_loaded():
                    raise RuntimeError("Whisper model not loaded")

                # ULTRA-enhanced medical prompt for Polish veterinary transcription
                # Guides Whisper towards medical vocabulary, reducing hallucinations
                medical_prompt = (
                    "Polski tekst weterynaryjny: diagnoza chorób zwierząt, objawy kliniczne, "
                    "leczenie farmakologiczne, badania laboratoryjne, RTG, USG, szczepienia, "
                    "odrobaczanie, wizyty kontrolne, wyniki badań krwi, temperatura ciała, "
                    "receptury leków, dawkowanie, rokowanie, zalecenia pielęgnacyjne."
                )

                result = whisper.transcribe(  # type: ignore[attr-defined]
                    path,
                    path_or_hf_repo=WHISPER_DIR,
                    verbose=True,  # Show language detection!
                    language=lang,
                    condition_on_previous_text=False,  # Critical for short clips!
                    initial_prompt=medical_prompt,
                    # Anti-hallucination filters (improves transcription quality)
                    compression_ratio_threshold=2.0,  # Lower = stricter (default 2.4)
                    no_speech_threshold=0.5,  # Higher = stricter (default 0.6)
                    logprob_threshold=-0.5,  # Higher = stricter (default -1.0)
                )
                # Log detected language for debugging
                if isinstance(result, dict):
                    detected = result.get("language", "unknown")
                    logging.info(f"Whisper detected language: {detected}")
                return result

            result = await asyncio.get_event_loop().run_in_executor(None, _do_transcribe)
            text = (result.get("text") or "").strip()
            logging.info(f"Transcription successful. Length: {len(text)} chars.")
            return {"ok": True, "text": text}
    except Exception as e:
        logging.error(f"Error during local transcription: {e}", exc_info=True)
        return {"ok": False, "error": str(e), "code": "local"}
    finally:
        if temp_path and os.path.exists(temp_path):
            try:
                os.remove(temp_path)
            except Exception as exc:
                logger.debug("Suppressed exception", exc_info=exc)
