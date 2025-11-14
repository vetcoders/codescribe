"""Session memory for VistaScribe - maintains context across transcriptions."""

import json
import logging
import uuid
from datetime import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)

# Session directory
SESSION_DIR = Path.home() / ".vistascribe" / "sessions"
SESSION_DIR.mkdir(parents=True, exist_ok=True)

# Current session storage
_current_session: Optional["TranscriptionSession"] = None


class TranscriptionSession:
    """Manages a single transcription session with memory."""

    def __init__(self, session_id: str = None):
        """Initialize a new session."""
        self.session_id = session_id or str(uuid.uuid4())
        self.start_time = datetime.now()
        self.transcripts: list[dict] = []  # List of {time, raw, formatted, assistive}
        self.context_window = []  # Recent context for AI
        self.session_file = SESSION_DIR / f"{self.session_id}.json"
        self.max_context_chars = 100_000  # ~25k tokens for Qwen

        logger.info(f"Session initialized: {self.session_id}")

    def add_transcript(
        self, raw_text: str, formatted_text: str = None, assistive: bool = False
    ) -> None:
        """Add a transcript to the session memory."""
        entry = {
            "timestamp": datetime.now().isoformat(),
            "raw": raw_text,
            "formatted": formatted_text or raw_text,
            "assistive": assistive,
        }
        self.transcripts.append(entry)

        # Update context window (keep recent history)
        self._update_context_window(entry)

        # Auto-save session
        self.save()

        logger.debug(f"Added transcript #{len(self.transcripts)} to session")

    def _update_context_window(self, entry: dict) -> None:
        """Maintain a sliding window of recent context."""
        # Add new entry
        self.context_window.append(entry)

        # Trim to max size (keep most recent)
        total_chars = sum(len(e.get("formatted", "")) for e in self.context_window)
        while total_chars > self.max_context_chars and len(self.context_window) > 1:
            removed = self.context_window.pop(0)
            total_chars -= len(removed.get("formatted", ""))

    def get_context_for_ai(self) -> str:
        """Get formatted context for AI processing."""
        if not self.context_window:
            return ""

        # Build context string
        context_parts = []
        context_parts.append(f"=== Session {self.session_id[:8]} ===")
        context_parts.append(f"Started: {self.start_time.strftime('%Y-%m-%d %H:%M')}")
        context_parts.append(f"Transcripts in session: {len(self.transcripts)}")
        context_parts.append("\n=== Recent Context ===")

        for entry in self.context_window[-10:]:  # Last 10 entries
            time = entry["timestamp"].split("T")[1].split(".")[0]
            text = entry["formatted"][:500]  # Truncate long entries
            mode = "🤖" if entry["assistive"] else "📝"
            context_parts.append(f"[{time}] {mode} {text}")

        context_parts.append("\n=== Current Input ===")
        return "\n".join(context_parts)

    def save(self) -> None:
        """Save session to disk."""
        try:
            data = {
                "session_id": self.session_id,
                "start_time": self.start_time.isoformat(),
                "transcripts": self.transcripts[-100:],  # Keep last 100
            }
            with open(self.session_file, "w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2)
            logger.debug(f"Session saved: {self.session_file}")
        except Exception as e:
            logger.error(f"Failed to save session: {e}")

    def load(self) -> bool:
        """Load session from disk."""
        try:
            if not self.session_file.exists():
                return False

            with open(self.session_file, encoding="utf-8") as f:
                data = json.load(f)

            self.session_id = data["session_id"]
            self.start_time = datetime.fromisoformat(data["start_time"])
            self.transcripts = data["transcripts"]

            # Rebuild context window
            for entry in self.transcripts:
                self._update_context_window(entry)

            logger.info(f"Session loaded: {self.session_id} ({len(self.transcripts)} transcripts)")
            return True
        except Exception as e:
            logger.error(f"Failed to load session: {e}")
            return False


def get_current_session() -> TranscriptionSession:
    """Get or create the current session."""
    global _current_session

    if _current_session is None:
        _current_session = TranscriptionSession()

        # Try to resume last session if recent
        last_session_file = _find_last_session()
        if last_session_file:
            temp_session = TranscriptionSession()
            temp_session.session_file = last_session_file
            if temp_session.load():
                # Resume if less than 1 hour old
                time_diff = datetime.now() - temp_session.start_time
                if time_diff.total_seconds() < 3600:
                    _current_session = temp_session
                    logger.info(f"Resumed session: {temp_session.session_id}")

    return _current_session


def _find_last_session() -> Path | None:
    """Find the most recent session file."""
    try:
        session_files = list(SESSION_DIR.glob("*.json"))
        if not session_files:
            return None
        return max(session_files, key=lambda p: p.stat().st_mtime)
    except Exception:
        return None


def new_session() -> TranscriptionSession:
    """Start a new session."""
    global _current_session
    _current_session = TranscriptionSession()
    logger.info(f"New session started: {_current_session.session_id}")
    return _current_session


def get_session_stats() -> dict:
    """Get statistics for the current session."""
    session = get_current_session()
    return {
        "session_id": session.session_id[:8],
        "duration": str(datetime.now() - session.start_time).split(".")[0],
        "transcripts": len(session.transcripts),
        "context_size": len(session.context_window),
    }
