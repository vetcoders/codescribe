"""Persistent settings shared between the tray and bundled app.

Settings live outside of the repo so that the packaged app and local dev
environment read/write the exact same JSON file. By default we keep them in
``$HOME/.VistaScribe/settings.json`` so CLI runs and the bundled app share the
exact same preferences regardless of platform.
"""

from __future__ import annotations

import json
import os
import threading
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .path_utils import user_data_root


@dataclass
class VistaSettings:
    language: str = "auto"
    ai_formatting_enabled: bool = False
    ai_provider: str = "harmony"  # harmony | ollama
    ai_max_tokens: int = 512
    ai_assistive_max_tokens: int = 2048


DEFAULT_SETTINGS = VistaSettings()
_lock = threading.Lock()
_cached: VistaSettings | None = None


def _settings_path() -> Path:
    custom = os.environ.get("VISTASCRIBE_SETTINGS_PATH")
    if custom:
        return Path(custom).expanduser()
    return user_data_root() / "settings.json"


def _ensure_dir(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)


def _load_from_disk(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}


def _sanitize(data: Mapping[str, Any]) -> VistaSettings:
    merged = asdict(DEFAULT_SETTINGS)
    merged.update({k: v for k, v in data.items() if k in merged})
    merged["language"] = str(merged["language"]).lower() or "auto"
    merged["ai_provider"] = str(merged["ai_provider"]).lower() or "harmony"
    if merged["ai_provider"] not in {"harmony", "ollama"}:
        merged["ai_provider"] = "harmony"
    merged["ai_formatting_enabled"] = bool(merged["ai_formatting_enabled"])
    merged["ai_max_tokens"] = int(merged["ai_max_tokens"] or 512)
    merged["ai_assistive_max_tokens"] = int(merged["ai_assistive_max_tokens"] or 2048)
    return VistaSettings(**merged)


def get_settings(force_reload: bool = False) -> VistaSettings:
    global _cached
    with _lock:
        if _cached is None or force_reload:
            path = _settings_path()
            data = _load_from_disk(path)
            _cached = _sanitize(data)
        return _cached


def save_settings(settings: VistaSettings) -> VistaSettings:
    with _lock:
        path = _settings_path()
        _ensure_dir(path)
        path.write_text(
            json.dumps(asdict(settings), indent=2, ensure_ascii=False), encoding="utf-8"
        )
        global _cached
        _cached = settings
        return settings


def update_settings(updates: Mapping[str, Any]) -> VistaSettings:
    current = get_settings()
    data = asdict(current)
    data.update(updates)
    return save_settings(_sanitize(data))


def reset_settings_for_tests(delete_file: bool = False):  # pragma: no cover - helper for tests
    global _cached
    with _lock:
        _cached = None
        if delete_file:
            path = _settings_path()
            if path.exists():
                path.unlink()
