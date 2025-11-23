"""CodeScribe context - local folder per project for memory."""

import logging
from datetime import datetime
from pathlib import Path

logger = logging.getLogger(__name__)


# Context folder - LOCAL to current working directory
def get_context_dir() -> Path:
    """Get or create .codescribe folder in current working directory."""
    context_dir = Path.cwd() / ".codescribe"
    context_dir.mkdir(exist_ok=True)
    return context_dir


# Max files to inject into prompt
MAX_CONTEXT_FILES = 20


def save_to_codescribe(
    raw_text: str, formatted_text: str | None = None, assistive: bool = False
) -> Path | None:
    """Save transcript to local .codescribe folder."""
    try:
        context_dir = get_context_dir()

        # Generate filename with timestamp
        timestamp = datetime.now().strftime("%Y%m%d-%H%M%S%f")[:-3]
        mode = "ai" if assistive else "fmt"
        filename = f"{timestamp}-{mode}.txt"
        filepath = context_dir / filename

        # Save transcript
        content = f"[{datetime.now().strftime('%H:%M:%S')}] "
        if formatted_text and formatted_text != raw_text:
            content += formatted_text
        else:
            content += raw_text
        content += "\n"

        filepath.write_text(content, encoding="utf-8")
        logger.debug(f"Saved to .codescribe: {filename}")
        return filepath

    except Exception as e:
        logger.error(f"Failed to save to .codescribe: {e}")
        return None


def inject_context_to_prompt(user_text: str) -> str:
    """Read .codescribe folder and PREPEND context to user's prompt.

    This is THE KEY - we inject previous conversations into the prompt
    so the model "remembers" what we talked about.
    """
    try:
        context_dir = get_context_dir()

        # Get all .txt files sorted by name (timestamp)
        files = sorted(context_dir.glob("*.txt"))

        if not files:
            # No history - return original text
            return user_text

        # Take last N files
        recent_files = files[-MAX_CONTEXT_FILES:] if len(files) > MAX_CONTEXT_FILES else files

        # Build context that we'll INJECT before user's text
        context_parts = []
        context_parts.append("=== PREVIOUS CONTEXT FROM .codescribe ===")

        for file in recent_files:
            try:
                content = file.read_text(encoding="utf-8").strip()
                if content:
                    context_parts.append(content)
            except Exception:
                continue

        context_parts.append("=== CURRENT INPUT ===")
        context_parts.append(user_text)

        # This is what model actually sees - full context + current input
        full_prompt = "\n".join(context_parts)

        logger.debug(f"Injected {len(recent_files)} files from .codescribe into prompt")
        logger.debug(f"Total prompt size: {len(full_prompt)} chars")

        return full_prompt

    except Exception as e:
        logger.error(f"Failed to inject context: {e}")
        # On error, just return original text
        return user_text


def get_codescribe_stats() -> dict:
    """Get stats about current .codescribe folder."""
    try:
        context_dir = get_context_dir()
        files = list(context_dir.glob("*.txt"))

        return {"location": str(context_dir), "files": len(files), "exists": context_dir.exists()}
    except Exception:
        return {"location": "error", "files": 0, "exists": False}
