from __future__ import annotations

import logging
import os
from collections.abc import Mapping
from dataclasses import dataclass

from .path_utils import repo_root
from .settings_store import get_settings, update_settings

logger = logging.getLogger(__name__)

_REPO_ROOT = str(repo_root())


@dataclass
class Config:
    whisper_url: str  # empty = local
    llm_url: str  # Harmony-compatible base URL
    format_enabled: bool
    language: str | None  # 'pl', 'en', or None for auto
    ai_provider: str = "harmony"


def _truthy(val: str | None) -> bool:
    if val is None:
        return False
    return val.strip().lower() not in {"0", "false", "no", "off", ""}


def load_config(env: Mapping[str, str] | None = None) -> Config:
    e = env or os.environ
    settings = get_settings()

    # Sanitize inputs: ensure strings
    def _get_str(key: str, default: str = "") -> str:
        val = e.get(key, default)
        if not isinstance(val, str):
            return default
        return val

    whisper_raw = _get_str("WHISPER_SERVER_URL", "").strip()
    llm_raw = _get_str("LLM_SERVER_URL", "").strip()
    lang_env = _get_str("WHISPER_LANGUAGE", "").strip().lower()
    lang_raw = settings.language if settings.language not in {"", "auto"} else lang_env
    return Config(
        whisper_url=whisper_raw,
        llm_url=llm_raw,
        format_enabled=settings.ai_formatting_enabled,
        language=(lang_raw or None),
        ai_provider=settings.ai_provider,
    )


def _read_env_file(path: str) -> dict[str, str]:
    env: dict[str, str] = {}
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.rstrip("\n")
                if not line or line.strip().startswith("#"):
                    continue
                if "=" in line:
                    k, v = line.split("=", 1)
                    env[k.strip()] = v
    except FileNotFoundError:
        logger.debug("Env file missing, skipping load: %s", path)
    return env


def _write_env_file(path: str, data: Mapping[str, str]) -> None:
    with open(path, "w", encoding="utf-8") as f:
        for k, v in data.items():
            f.write(f"{k}={v}\n")


def serialize_env(cfg: Config, base: Mapping[str, str] | None = None) -> str:
    env: dict[str, str] = dict(base or {})
    wurl = cfg.whisper_url if isinstance(cfg.whisper_url, str) else ""
    lurl = cfg.llm_url if isinstance(cfg.llm_url, str) else ""
    lang = cfg.language if isinstance(cfg.language, str) else ""
    env.update(
        {
            "WHISPER_SERVER_URL": wurl,
            "LLM_SERVER_URL": lurl,
            "WHISPER_LANGUAGE": lang,
        }
    )
    # Deterministic order: core keys first, then others sorted
    core_keys = [
        "WHISPER_SERVER_URL",
        "LLM_SERVER_URL",
        "WHISPER_LANGUAGE",
    ]
    lines: list[str] = []
    for k in core_keys:
        lines.append(f"{k}={env.pop(k, '')}")
    for k in sorted(env.keys()):
        lines.append(f"{k}={env[k]}")
    return "\n".join(lines) + "\n"


def save_config(cfg: Config, path: str | None = None) -> None:
    p = path or os.path.join(_REPO_ROOT, ".env")
    base = _read_env_file(p)
    content = serialize_env(cfg, base)
    _write_env_file(
        p, _read_env_file(p) | dict(line.split("=", 1) for line in content.strip().split("\n"))
    )
    update_settings({"ai_formatting_enabled": bool(cfg.format_enabled)})


def update_env_vars(updates: Mapping[str, str], path: str | None = None) -> None:
    """Merge selected env vars into the .env file (preserves others)."""
    p = path or os.path.join(_REPO_ROOT, ".env")
    env = _read_env_file(p)
    env.update({k: str(v) for k, v in updates.items()})
    # Keep order stable: core keys first if present, then others sorted
    core = [
        "WHISPER_SERVER_URL",
        "LLM_SERVER_URL",
        "WHISPER_LANGUAGE",
    ]
    ordered: dict[str, str] = {}
    for k in core:
        if k in env:
            ordered[k] = env.pop(k)
    for k in sorted(env.keys()):
        ordered[k] = env[k]
    _write_env_file(p, ordered)
