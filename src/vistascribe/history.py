"""Simple transcript history manager for VistaScribe."""

import logging
import subprocess
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from .path_utils import user_data_root


@dataclass
class HistoryEntry:
    path: Path
    timestamp: datetime
    preview: str

    @property
    def label(self) -> str:
        ts = self.timestamp.strftime("%H:%M:%S")
        return f"{ts} – {self.preview}" if self.preview else ts


def history_dir() -> Path:
    dir_path = user_data_root() / "Transcripts"
    dir_path.mkdir(parents=True, exist_ok=True)
    return dir_path


def save_entry(text: str) -> HistoryEntry:
    text = (text or "").strip()
    ts = datetime.now()
    day_dir = history_dir() / ts.strftime("%Y-%m-%d")
    day_dir.mkdir(parents=True, exist_ok=True)
    filename = ts.strftime("%H%M%S.txt")
    path = day_dir / filename
    try:
        path.write_text(text, encoding="utf-8")
    except Exception as exc:  # pragma: no cover - depends on FS perms
        logging.error(f"Failed to write transcript history '{path}': {exc}")
    preview = text.splitlines()[0][:60] if text else ""
    return HistoryEntry(path=path, timestamp=ts, preview=preview)


def recent_entries(limit: int = 5) -> list[HistoryEntry]:
    dir_path = history_dir()
    files = sorted(dir_path.rglob("*.txt"), key=lambda p: p.stat().st_mtime, reverse=True)
    entries: list[HistoryEntry] = []
    for path in files[:limit]:
        try:
            ts = datetime.fromtimestamp(path.stat().st_mtime)
        except Exception:
            ts = datetime.now()
        try:
            preview = path.read_text(encoding="utf-8", errors="replace").strip()
        except Exception:
            preview = ""
        preview = preview.splitlines()[0][:60] if preview else ""
        entries.append(HistoryEntry(path=path, timestamp=ts, preview=preview))
    return entries


def open_history_folder() -> None:
    dir_path = history_dir()
    try:
        subprocess.run(["open", str(dir_path)], check=False)
    except Exception as exc:  # pragma: no cover
        logging.error(f"Failed to open history folder: {exc}")


def clear_history() -> None:
    dir_path = history_dir()
    for path in dir_path.rglob("*.txt"):
        try:
            path.unlink()
        except Exception as exc:
            logging.warning(f"Failed to delete history entry '{path}': {exc}")
            continue
