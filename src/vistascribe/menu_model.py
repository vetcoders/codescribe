"""
menu_model.py — Declarative menu specifications and safe rendering helpers.

This module keeps data structures pure (no rumps import required) so it can be
unit-tested without macOS/PyObjC. The renderer that turns specs into rumps
objects imports rumps lazily at runtime in the app process.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from .event_log import instrument_menu_item


@dataclass(frozen=True)
class MenuSpecItem:
    kind: str  # 'label' | 'action' | 'sep'
    title: str | None = None
    action: str | None = None  # action key for callbacks mapping


def build_models_spec(
    is_remote: bool,
    current_label: str,
    ollama_models: list[str] | None = None,
    current_ollama: str | None = None,
) -> list[MenuSpecItem]:
    """Return a declarative spec for the Models submenu.

    When is_remote is True (WHISPER_SERVER_URL set), we prune local-only actions.
    ollama_models: Optional list of available Ollama models.
    current_ollama: Currently selected Ollama model.
    """
    import os

    items: list[MenuSpecItem] = []

    # Whisper models section
    items.append(MenuSpecItem(kind="label", title=f"Whisper: {current_label}"))
    if not is_remote:
        items.extend(
            [
                MenuSpecItem(kind="sep"),
                MenuSpecItem(kind="action", title="Use Whisper: Small", action="use_small"),
                MenuSpecItem(kind="action", title="Use Whisper: Medium", action="use_medium"),
                MenuSpecItem(kind="action", title="Use Whisper: Large v3", action="use_lv3"),
                MenuSpecItem(kind="action", title="Use Whisper: Large v3 Turbo", action="use_lvt"),
            ]
        )

    # Ollama models section
    if ollama_models or current_ollama:
        items.append(MenuSpecItem(kind="sep"))

        # Show current Ollama model
        if not current_ollama:
            current_ollama = os.environ.get("OLLAMA_MODEL", "gpt-oss:120b")
        current_display = current_ollama.split("/")[-1] if "/" in current_ollama else current_ollama
        if len(current_display) > 25:
            current_display = current_display[:22] + "..."
        items.append(MenuSpecItem(kind="label", title=f"Ollama: {current_display}"))

        if ollama_models:
            items.append(MenuSpecItem(kind="sep"))
            for model in ollama_models[:10]:  # Limit to first 10 models
                # Shorten model name for display
                display_name = model.split("/")[-1] if "/" in model else model
                if len(display_name) > 30:
                    display_name = display_name[:27] + "..."
                # Mark current model with checkmark
                title = f"✓ {display_name}" if model == current_ollama else f"  {display_name}"
                items.append(MenuSpecItem(kind="action", title=title, action=f"ollama_{model}"))

    items.extend(
        [
            MenuSpecItem(kind="sep"),
            MenuSpecItem(kind="action", title="Open Models Folder", action="open_models"),
        ]
    )
    return items


def render_rumps_menu(
    app,
    spec: list[MenuSpecItem],
    actions: dict[str, Callable[[], None]],
    *,
    alert_title: str = "VistaScribe",
):
    """Render spec to a list of rumps.MenuItem objects (or None for separators).

    Wrap each action in a safe callback so UI exceptions never crash the app.
    """
    import rumps  # lazy import

    def _safe_call(fn: Callable[[], None]):
        def _wrapper(_sender):
            try:
                fn()
            except Exception as e:  # pragma: no cover (UI-only)
                import logging

                logging.error(f"Menu action failed: {e}")
                # DON'T show rumps.alert - it can freeze menu rendering!
                # User will see error in logs instead

        return _wrapper

    out: list[rumps.MenuItem | None] = []  # type: ignore[name-defined]
    for entry in spec:
        if entry.kind == "sep":
            out.append(None)
        elif entry.kind == "label":
            out.append(rumps.MenuItem(entry.title or ""))
        elif entry.kind == "action":
            cb = actions.get(entry.action or "")
            if cb is None:
                # render disabled item when action is missing
                item = rumps.MenuItem(entry.title or "(missing)")
                item.set_callback(lambda _s: None)
                instrument_menu_item(item)
                out.append(item)
            else:
                item = rumps.MenuItem(entry.title or "", callback=_safe_call(cb))
                instrument_menu_item(item)
                out.append(item)
        else:
            # unknown kind -> skip
            out.append(None)
    return out
