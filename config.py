from __future__ import annotations

import os
from collections.abc import Mapping
from dataclasses import dataclass


@dataclass
class Config:
    whisper_url: str  # empty = local
    llm_url: str      # empty = local
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


def serialize_env(cfg: Config) -> str:
    lines: list[str] = []
    wurl = cfg.whisper_url if isinstance(cfg.whisper_url, str) else ""
    lurl = cfg.llm_url if isinstance(cfg.llm_url, str) else ""
    lang = cfg.language if isinstance(cfg.language, str) else ""
    lines.append(f"WHISPER_SERVER_URL={wurl}")
    lines.append(f"LLM_SERVER_URL={lurl}")
    lines.append(f"FORMAT_ENABLED={'1' if cfg.format_enabled else '0'}")
    lines.append(f"WHISPER_LANGUAGE={lang}")
    return "\n".join(lines) + "\n"


def save_config(cfg: Config, path: str | None = None) -> None:
    p = path or os.path.join(os.path.dirname(os.path.abspath(__file__)), ".env")
    content = serialize_env(cfg)
    with open(p, "w", encoding="utf-8") as f:
        f.write(content)
