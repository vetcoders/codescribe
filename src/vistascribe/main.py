"""VistaScribe tray entry point.

This module intentionally stays tiny and hands off all heavy lifting to
`vistascribe.app.runtime` so the historical import path keeps working while the
runtime lives in a dedicated module.
"""

from __future__ import annotations

from .app.runtime import VistaScribe, acquire_lock, run

__all__ = ["VistaScribe", "acquire_lock", "run", "main"]


def main() -> None:
    """Boot the VistaScribe tray application."""
    run()


if __name__ == "__main__":
    main()
