"""CodeScribe core Python package."""

from __future__ import annotations

try:
    from importlib.metadata import PackageNotFoundError, version as _version_func
except Exception:  # pragma: no cover - stdlib import
    _version_func = None  # type: ignore[assignment]

    class PackageNotFoundError(Exception):  # type: ignore[no-redef]
        pass


__all__ = ["__version__"]

if _version_func is not None:
    try:
        __version__ = _version_func("CodeScribe")
    except PackageNotFoundError:  # pragma: no cover - dev mode
        __version__ = "0.0.0-dev"
else:  # pragma: no cover - extremely old Python
    __version__ = "0.0.0-dev"
