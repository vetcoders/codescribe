from __future__ import annotations

import os
from collections.abc import Mapping
from dataclasses import dataclass


@dataclass
class Config:
    whisper_url: str  # empty = local
    llm_url: str  # empty = local
    format_enabled: bool
    language: str | None  # 'pl', 'en', or None for auto


def _truthy(val: str | None) -> bool:
    if val is None:
        return False
    return val.strip().lower() not in {"0", "false", "no", "off", ""}


def load_config(env: Mapping[str, str] | None = None) -> Config:
    e = env or os.environ

    # Sanitize inputs: ensure strings
    def _get_str(key: str, default: str = "") -> str:
        val = e.get(key, default)
        if not isinstance(val, str):
            return default
        return val

    whisper_raw = _get_str("WHISPER_SERVER_URL", "").strip()
    llm_raw = _get_str("LLM_SERVER_URL", "").strip()
    fmt_raw = _get_str("FORMAT_ENABLED", "0")  # default disabled to match llm.py
    lang_raw = _get_str("WHISPER_LANGUAGE", "").strip().lower()
    return Config(
        whisper_url=whisper_raw,
        llm_url=llm_raw,
        format_enabled=_truthy(fmt_raw),
        language=(lang_raw or None),
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
        pass
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
            "FORMAT_ENABLED": "1" if cfg.format_enabled else "0",
            "WHISPER_LANGUAGE": lang,
        }
    )
    # Deterministic order: core keys first, then others sorted
    core_keys = [
        "WHISPER_SERVER_URL",
        "LLM_SERVER_URL",
        "FORMAT_ENABLED",
        "WHISPER_LANGUAGE",
    ]
    lines: list[str] = []
    for k in core_keys:
        lines.append(f"{k}={env.pop(k, '')}")
    for k in sorted(env.keys()):
        lines.append(f"{k}={env[k]}")
    return "\n".join(lines) + "\n"


def save_config(cfg: Config, path: str | None = None) -> None:
    p = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), ".env")
    base = _read_env_file(p)
    content = serialize_env(cfg, base)
    _write_env_file(
        p,
        _read_env_file(p)
        | dict([tuple(line.split("=", 1)) for line in content.strip().split("\n")]),
    )


def update_env_vars(updates: Mapping[str, str], path: str | None = None) -> None:
    """Merge selected env vars into the .env file (preserves others)."""
    p = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), ".env")
    env = _read_env_file(p)
    env.update({k: str(v) for k, v in updates.items()})
    # Keep order stable: core keys first if present, then others sorted
    core = [
        "WHISPER_SERVER_URL",
        "LLM_SERVER_URL",
        "FORMAT_ENABLED",
        "WHISPER_LANGUAGE",
    ]
    ordered: dict[str, str] = {}
    for k in core:
        if k in env:
            ordered[k] = env.pop(k)
    for k in sorted(env.keys()):
        ordered[k] = env[k]
    _write_env_file(p, ordered)
