"""Persistent settings shared between the tray and bundled app.

Settings are now read EXCLUSIVELY from environment variables loaded via .env files.
The primary .env location is ``$HOME/.CodeScribe/.env`` which is loaded by backend.py
and llm.py at startup using python-dotenv.

This module provides a read-only view of the current environment configuration.
There is NO settings.json - only .env is the source of truth.
"""

from __future__ import annotations

import logging
import os
import threading
from dataclasses import dataclass

logger = logging.getLogger(__name__)


def _env_bool(key: str, default: bool = False) -> bool:
    """Parse a boolean from environment variable.

    Truthy values: 1, true, yes, on (case-insensitive)
    Falsy values: 0, false, no, off, empty string (case-insensitive)
    """
    val = os.environ.get(key, "").strip().lower()
    if not val:
        return default
    return val in ("1", "true", "yes", "on")


def _env_int(key: str, default: int) -> int:
    """Parse an integer from environment variable."""
    val = os.environ.get(key, "").strip()
    if not val:
        return default
    try:
        return int(val)
    except ValueError:
        logger.warning("Invalid integer for %s: %r, using default %d", key, val, default)
        return default


def _env_str(key: str, default: str = "") -> str:
    """Get a string from environment variable."""
    return os.environ.get(key, default).strip() or default


@dataclass
class VistaSettings:
    """Read-only view of current settings from environment variables."""

    language: str = "auto"
    ai_formatting_enabled: bool = False
    ai_provider: str = "harmony"  # harmony | ollama
    ai_max_tokens: int = 512
    ai_assistive_max_tokens: int = 2048
    # Additional settings from .env
    history_enabled: bool = True
    agent_name: str = "asystent"
    hold_mods: str = "ctrl"
    toggle_trigger: str = "double_ralt"


# Cache settings to avoid repeated env lookups (can be invalidated with force_reload)
_lock = threading.Lock()
_cached: VistaSettings | None = None


def _build_settings_from_env() -> VistaSettings:
    """Build VistaSettings from current environment variables.

    Environment variable mapping:
        AI_FORMATTING_ENABLED   -> ai_formatting_enabled (bool)
        AI_ASSISTIVE_MAX_TOKENS -> ai_assistive_max_tokens (int)
        AI_MAX_TOKENS           -> ai_max_tokens (int)
        AI_PROVIDER             -> ai_provider (str: harmony|ollama)
        LANGUAGE                -> language (str)
        HISTORY_ENABLED         -> history_enabled (bool)
        AGENT_NAME              -> agent_name (str)
        HOLD_MODS               -> hold_mods (str)
        TOGGLE_TRIGGER          -> toggle_trigger (str)
    """
    ai_provider = _env_str("AI_PROVIDER", "harmony").lower()
    if ai_provider not in {"harmony", "ollama"}:
        ai_provider = "harmony"

    return VistaSettings(
        language=_env_str("LANGUAGE", "auto").lower() or "auto",
        ai_formatting_enabled=_env_bool("AI_FORMATTING_ENABLED", False),
        ai_provider=ai_provider,
        ai_max_tokens=_env_int("AI_MAX_TOKENS", 512),
        ai_assistive_max_tokens=_env_int("AI_ASSISTIVE_MAX_TOKENS", 2048),
        history_enabled=_env_bool("HISTORY_ENABLED", True),
        agent_name=_env_str("AGENT_NAME", "asystent"),
        hold_mods=_env_str("HOLD_MODS", "ctrl"),
        toggle_trigger=_env_str("TOGGLE_TRIGGER", "double_ralt"),
    )


def get_settings(force_reload: bool = False) -> VistaSettings:
    """Get current settings from environment variables.

    Settings are cached for performance. Use force_reload=True to re-read
    from environment (e.g., after modifying os.environ at runtime).
    """
    global _cached
    with _lock:
        if _cached is None or force_reload:
            _cached = _build_settings_from_env()
        return _cached


def save_settings(settings: VistaSettings) -> VistaSettings:
    """No-op: Settings are read-only from .env.

    This function exists for backward compatibility but does nothing.
    To change settings, edit ~/.CodeScribe/.env and restart the backend.
    """
    logger.warning(
        "save_settings() called but settings are now read-only from .env. "
        "Edit ~/.CodeScribe/.env and restart to change settings."
    )
    return settings


def update_settings(updates: dict[str, object]) -> VistaSettings:
    """No-op: Settings are read-only from .env.

    This function exists for backward compatibility but does nothing.
    To change settings, edit ~/.CodeScribe/.env and restart the backend.
    """
    logger.warning(
        "update_settings() called but settings are now read-only from .env. "
        "Edit ~/.CodeScribe/.env and restart to change settings."
    )
    return get_settings()


def reset_settings_for_tests(delete_file: bool = False):  # pragma: no cover - helper for tests
    """Reset cached settings (useful for tests that modify os.environ)."""
    global _cached
    with _lock:
        _cached = None
    # delete_file parameter is ignored since we no longer use settings.json
