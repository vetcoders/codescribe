"""VistaScribe core Python package."""

from __future__ import annotations

try:
    from importlib.metadata import PackageNotFoundError, version
except Exception:  # pragma: no cover - stdlib import
    version = None  # type: ignore[assignment]
    PackageNotFoundError = Exception  # type: ignore[assignment]

__all__ = ["__version__"]

if version:
    try:
        __version__ = version("VistaScribe")
    except PackageNotFoundError:  # pragma: no cover - dev mode
        __version__ = "0.0.0-dev"
else:  # pragma: no cover - extremely old Python
    __version__ = "0.0.0-dev"
