import logging

logger = logging.getLogger(__name__)
"""
model_defaults.py - Base model configuration for VistaScribe

IMPORTANT: This file is for DOCUMENTATION ONLY - not imported by the app!
All model configuration comes from .env file.

Base models (bundled with binary for survival only):
- Whisper: small (base STT)
- LLM: qwen3:4b MXFP4 quantized (base formatting/AI)

Everything else is user configurable via .env - NO AUTO-DETECTION!
"""

# Base models - these ship with the app
BASE_WHISPER_MODEL = "whisper-small"
BASE_LLM_MODEL = "qwen3:4b"
BASE_LLM_MLX_PATH = "models/qwen3-4b-mxfp4-mlx"  # Will be created

# Default endpoints - user configurable
# Team uses Dragon via Tailscale: http://100.82.232.70:11434
DEFAULT_OLLAMA_HOST = "http://127.0.0.1:11434"  # Local fallback only

# Model hierarchy for auto-detection
WHISPER_MODELS = [
    "whisper-large-v3-turbo",  # Best if available
    "whisper-large-v3",
    "whisper-medium",
    "whisper-small",  # Base - always available
]

LLM_MODELS = [
    "qwen3-coder:30b",  # Dragon mode
    "qwen3:14b",  # High-end
    "qwen3:7b",  # Medium
    "qwen3:4b",  # Base - always available
]


def get_base_models_info():
    """Return info about bundled base models."""
    return {
        "stt": {"model": BASE_WHISPER_MODEL, "size": "~40MB", "quality": "good for most cases"},
        "llm": {
            "model": BASE_LLM_MODEL,
            "quantization": "MXFP4",
            "size": "~2GB after quantization",
            "quality": "excellent for formatting and basic AI",
        },
    }


def detect_best_models():
    """Detect best available models, fallback to base."""
    from pathlib import Path

    models_dir = Path("models")

    # Find best Whisper
    best_whisper = BASE_WHISPER_MODEL
    for model in WHISPER_MODELS:
        if (models_dir / model).exists():
            best_whisper = model
            break

    # For LLM, check Ollama first
    try:
        import subprocess

        result = subprocess.run(["ollama", "list"], capture_output=True, text=True)
        if "qwen3" in result.stdout:
            # Parse available qwen3 models
            for line in result.stdout.split("\n"):
                for model in LLM_MODELS:
                    if model in line:
                        return best_whisper, model
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)

    # Fallback to MLX base model
    if (Path(BASE_LLM_MLX_PATH)).exists():
        return best_whisper, BASE_LLM_MLX_PATH

    return best_whisper, BASE_LLM_MODEL
