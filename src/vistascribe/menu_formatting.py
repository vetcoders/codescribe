"""Minimal formatting submenu: Light+ baseline + optional AI."""

from __future__ import annotations

import rumps

from .app.menu_utils import set_submenu
from .settings_store import get_settings


def build_formatting_menu(app, parent_menu: rumps.MenuItem):
    """Build the formatting submenu with a single toggle and provider picker."""

    settings = get_settings(force_reload=True)
    provider_cb = getattr(app, "_set_ai_provider", None) or getattr(
        app, "_set_format_strategy", None
    )

    def _noop(_sender):
        rumps.alert(title="VistaScribe", message="Handler not wired for this build")

    cb_provider = provider_cb or _noop

    entries = [
        rumps.MenuItem("Provider", callback=lambda _s: None),
        rumps.MenuItem(
            "✓ Harmony" if settings.ai_provider == "harmony" else "  Harmony",
            callback=lambda _s: cb_provider("harmony"),
        ),
        rumps.MenuItem(
            "✓ Ollama" if settings.ai_provider == "ollama" else "  Ollama",
            callback=lambda _s: cb_provider("ollama"),
        ),
        None,
        rumps.MenuItem("Light+ always on", callback=lambda _s: None),
    ]

    set_submenu(parent_menu, entries)
    return parent_menu
