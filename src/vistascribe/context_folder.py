"""Simple folder-based context memory for VistaScribe - like Claude Code."""

import logging
from datetime import datetime
from pathlib import Path

logger = logging.getLogger(__name__)

# Context folder location
CONTEXT_DIR = Path.home() / ".vistascribe" / "context"
CONTEXT_DIR.mkdir(parents=True, exist_ok=True)

# Max files to read for context (newest first)
MAX_CONTEXT_FILES = 20


def save_transcript(raw_text: str, formatted_text: str = None, assistive: bool = False) -> Path:
    """Save transcript to context folder."""
    try:
        # Generate filename with timestamp
        timestamp = datetime.now().strftime("%Y%m%d-%H%M%S%f")[:-3]
        mode = "assistive" if assistive else "format"
        filename = f"{timestamp}-{mode}.txt"
        filepath = CONTEXT_DIR / filename

        # Save transcript with metadata
        content = f"=== {datetime.now().isoformat()} ===\n"
        content += f"Mode: {'Assistive AI' if assistive else 'Formatting'}\n"
        content += f"Raw: {raw_text}\n"
        if formatted_text:
            content += f"Formatted: {formatted_text}\n"
        content += "\n"

        filepath.write_text(content, encoding="utf-8")
        logger.debug(f"Saved transcript to context: {filename}")
        return filepath

    except Exception as e:
        logger.error(f"Failed to save transcript: {e}")
        return None


def get_context_for_llm(max_files: int = MAX_CONTEXT_FILES) -> str:
    """Read recent transcripts from folder for LLM context."""
    try:
        # Get all .txt files sorted by name (timestamp)
        files = sorted(CONTEXT_DIR.glob("*.txt"), reverse=True)[:max_files]

        if not files:
            return ""

        # Build context string
        context_parts = []
        context_parts.append(f"=== Session Context ({len(files)} recent transcripts) ===\n")

        for file in reversed(files):  # Show oldest first for chronological order
            try:
                content = file.read_text(encoding="utf-8")
                # Extract just the formatted text if available
                lines = content.split("\n")
                for line in lines:
                    if line.startswith("Formatted: "):
                        text = line[11:].strip()[:200]  # First 200 chars
                        time = file.stem.split("-")[1]  # Extract time from filename
                        context_parts.append(f"[{time[:2]}:{time[2:4]}:{time[4:6]}] {text}")
                        break
            except Exception:
                continue

        context_parts.append("\n=== Current Input ===")
        return "\n".join(context_parts)

    except Exception as e:
        logger.error(f"Failed to get context: {e}")
        return ""


def cleanup_old_files(days: int = 7):
    """Remove context files older than N days."""
    try:
        from datetime import timedelta

        cutoff = datetime.now() - timedelta(days=days)

        for file in CONTEXT_DIR.glob("*.txt"):
            # Parse timestamp from filename
            try:
                timestamp_str = file.stem.split("-")[0]
                file_date = datetime.strptime(timestamp_str, "%Y%m%d")
                if file_date < cutoff:
                    file.unlink()
                    logger.debug(f"Removed old context file: {file.name}")
            except Exception:
                continue

    except Exception as e:
        logger.error(f"Failed to cleanup old files: {e}")


def get_session_stats() -> dict:
    """Get simple stats about current session."""
    try:
        files = list(CONTEXT_DIR.glob("*.txt"))
        if not files:
            return {"transcripts": 0, "session": "empty"}

        # Get session start from oldest file today
        today = datetime.now().strftime("%Y%m%d")
        today_files = [f for f in files if f.name.startswith(today)]

        return {
            "transcripts": len(today_files),
            "total": len(files),
            "session": f"{len(today_files)} today",
        }
    except Exception:
        return {"transcripts": 0, "session": "error"}
