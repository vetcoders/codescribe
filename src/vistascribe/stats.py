"""Lightweight telemetry store for VistaScribe."""

from __future__ import annotations

import json
import logging
from dataclasses import asdict, dataclass
from datetime import datetime
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any

from .path_utils import user_data_root


@dataclass
class StatsSnapshot:
    total_transcripts: int = 0
    total_chars_raw: int = 0
    total_chars_formatted: int = 0
    total_seconds: float = 0.0
    change_sum: float = 0.0
    change_samples: int = 0
    updated_at: str = ""

    @property
    def change_ratio(self) -> float:
        if self.change_samples == 0:
            return 0.0
        return self.change_sum / self.change_samples


def _stats_dir() -> Path:
    path = user_data_root() / "Stats"
    path.mkdir(parents=True, exist_ok=True)
    return path


def _stats_file() -> Path:
    return _stats_dir() / "stats.json"


def _load_stats() -> StatsSnapshot:
    path = _stats_file()
    if not path.exists():
        return StatsSnapshot(updated_at=datetime.utcnow().isoformat())
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        logging.error("Failed to read stats file: %s", exc)
        return StatsSnapshot(updated_at=datetime.utcnow().isoformat())
    snapshot = StatsSnapshot(**{**StatsSnapshot().__dict__, **data})
    if not snapshot.updated_at:
        snapshot.updated_at = datetime.utcnow().isoformat()
    return snapshot


def _save_stats(snapshot: StatsSnapshot) -> None:
    snapshot.updated_at = datetime.utcnow().isoformat()
    try:
        _stats_file().write_text(json.dumps(asdict(snapshot), indent=2), encoding="utf-8")
    except Exception as exc:
        logging.error("Failed to write stats file: %s", exc)


def record_transcript(
    raw_text: str | None, formatted_text: str | None, duration_seconds: float
) -> None:
    """Update aggregate stats with a new transcript event."""

    snapshot = _load_stats()

    raw = raw_text or ""
    formatted = formatted_text or raw

    snapshot.total_transcripts += 1
    snapshot.total_chars_raw += len(raw)
    snapshot.total_chars_formatted += len(formatted)
    snapshot.total_seconds += max(duration_seconds or 0.0, 0.0)

    try:
        matcher = SequenceMatcher(None, raw, formatted)
        similarity = matcher.ratio()
        change = max(0.0, 1.0 - similarity)
    except Exception:
        change = 0.0
    if change > 0.0:
        snapshot.change_sum += change
        snapshot.change_samples += 1

    _save_stats(snapshot)


def load_snapshot() -> dict[str, Any]:
    """Return a dict suitable for UI consumption."""

    snapshot = _load_stats()
    data = asdict(snapshot)
    data["change_ratio"] = snapshot.change_ratio
    return data
