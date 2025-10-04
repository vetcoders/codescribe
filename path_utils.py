# path_utils.py
#
# Utilities to normalize model paths for MLX (mlx_lm / mlx_whisper).
# Some MLX tooling is picky about uppercase characters in absolute paths
# (e.g., '/Users' on macOS). This helper converts typical macOS paths
# to lowercase variants and expands user/relative paths safely.

from __future__ import annotations

import logging
import os

logger = logging.getLogger(__name__)


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
    if os.path.isabs(expanded) or expanded.startswith(('./', '../', '.')):
        abs_path = os.path.abspath(expanded)

        # Workaround: some MLX versions reject uppercase in absolute paths
        if abs_path.startswith('/Users/'):
            fixed = '/users/' + abs_path[len('/Users/') :]
            try:
                if os.path.exists(fixed):
                    if fixed != abs_path:
                        logger.info(f"Normalized path for MLX: '{abs_path}' -> '{fixed}'")
                    abs_path = fixed
            except Exception:
                pass  # keep original abs_path on any error
        return abs_path

    # Otherwise, this is likely a model repo ID (e.g., 'org/name'); return as-is
    return expanded
