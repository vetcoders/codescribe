# path_utils.py
#
# Utilities to normalize model paths for MLX (mlx_lm / mlx_whisper).
# Some MLX tooling is picky about uppercase characters in absolute paths
# (e.g., '/Users' on macOS). This helper converts typical macOS paths
# to lowercase variants and expands user/relative paths safely.

from __future__ import annotations

import logging
import os
from pathlib import Path

logger = logging.getLogger(__name__)


def _sanitize_log(value: str | None) -> str:
    """Sanitize user input for safe logging (prevent log injection)."""
    if value is None:
        return "<none>"
    return (
        str(value)
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace("\t", "\\t")
        .replace("\x00", "\\x00")
    )


_PACKAGE_ROOT = Path(__file__).resolve().parent
_SRC_ROOT = _PACKAGE_ROOT.parent
if _SRC_ROOT.name == "server" and (_SRC_ROOT.parent / "pyproject.toml").exists():
    _REPO_ROOT = _SRC_ROOT.parent
else:
    _REPO_ROOT = _SRC_ROOT


def package_root() -> Path:
    """Return the directory containing the packaged CodeScribe modules."""

    return _PACKAGE_ROOT


def repo_root() -> Path:
    """Return the checkout root if available, otherwise the install base."""

    return _REPO_ROOT


def normalize_model_path(p: str | None) -> str | None:
    """Return a normalized absolute path suitable for MLX.

    - Expands '~' and environment variables.
    - Converts filesystem-like inputs to absolute paths.
    - Replaces '/Users/' prefix with '/users/' (workaround for mlx path casing).
    - Leaves non-path identifiers (like HF repo IDs) unchanged.
    """
    if not p:
        return p

    # First expand user/home and env vars
    expanded = os.path.expandvars(os.path.expanduser(p))

    # If after expansion it looks like a filesystem path, absolutize it.
    # Treat as filesystem path when:
    #  - it is absolute OR
    #  - it starts with './' or '../' or '.' (relative path indicators)
    if os.path.isabs(expanded) or expanded.startswith(("./", "../", ".")):
        abs_path = os.path.abspath(expanded)

        # Workaround: some MLX versions reject uppercase in absolute paths
        if abs_path.startswith("/Users/"):
            fixed = "/users/" + abs_path[len("/Users/") :]
            try:
                if os.path.exists(fixed):
                    if fixed != abs_path:
                        # _sanitize_log() prevents log injection
                        safe_from = _sanitize_log(abs_path)
                        safe_to = _sanitize_log(fixed)
                        logger.info(f"Normalized path: '{safe_from}' -> '{safe_to}'")  # nosemgrep
                    abs_path = fixed
            except Exception as exc:
                logger.debug("Failed to normalize MLX path '%s': %s", _sanitize_log(abs_path), exc)
        return abs_path

    # Otherwise, this is likely a model repo ID (e.g., 'org/name'); return as-is
    return expanded


def user_data_root() -> Path:
    """Return the directory for user-scoped CodeScribe data.

    By default we store data inside ``$HOME/.CodeScribe`` on every platform so CLI
    runs and bundled apps share the same location. The path can be overridden with
    ``CODESCRIBE_DATA_DIR`` (preferred) or the legacy ``CODESCRIBE_APP_DIR`` for
    backwards compatibility.
    """

    for env in ("CODESCRIBE_DATA_DIR", "CODESCRIBE_APP_DIR"):
        custom = os.environ.get(env)
        if custom:
            return Path(os.path.expandvars(custom)).expanduser()

    return Path.home() / ".CodeScribe"
