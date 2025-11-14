"""History submenu controller."""

from __future__ import annotations

from pathlib import Path

import rumps

from ... import history
from ...ui import copy_text
from ..menu_utils import create_parent_item, set_submenu
from ..status import set_status


class HistoryController:
    def __init__(self, app) -> None:
        self.app = app
        self.menu = create_parent_item("History")
        self.item_label = rumps.MenuItem("Latest: —", callback=lambda _s: None)
        self.item_toggle = rumps.MenuItem(
            "Save transcripts to History", callback=self.toggle_history
        )
        self.item_copy_last = rumps.MenuItem(
            "Copy Latest to Clipboard", callback=lambda _s: self.copy_latest()
        )
        self.item_open = rumps.MenuItem(
            "Open History Folder", callback=lambda _s: history.open_history_folder()
        )
        self._latest_history_path: Path | None = None
        self.schedule_refresh()

    # ------------------------------------------------------------------
    def schedule_refresh(self) -> None:
        helper = getattr(rumps, "AppHelper", None)
        if helper is not None:
            helper.call_after(self.refresh)
        else:
            self.refresh()

    def refresh(self) -> None:
        app = self.app
        self.item_toggle.state = int(app.history_enabled)
        entries = history.recent_entries(5) if app.history_enabled else []

        if app.history_enabled and entries:
            latest = entries[0]
            preview = latest.preview or "(empty)"
            self.item_label.title = f"Latest: {latest.timestamp.strftime('%H:%M:%S')} – {preview}"
        elif app.history_enabled:
            self.item_label.title = "Latest: —"
        else:
            self.item_label.title = "History disabled"

        items: list[rumps.MenuItem | None] = [self.item_label, None, self.item_toggle, None]

        if app.history_enabled and entries:
            for entry in entries:
                label = entry.label or entry.timestamp.strftime("%H:%M:%S")
                items.append(
                    rumps.MenuItem(
                        label,
                        callback=lambda _s, p=entry.path: self.copy_entry(Path(p)),
                    )
                )
            items.append(None)
        elif app.history_enabled:
            items.append(rumps.MenuItem("No history yet", callback=lambda _s: None))
            items.append(None)
        else:
            items.append(rumps.MenuItem("Transcript saving is off", callback=lambda _s: None))
            items.append(None)

        if app.history_enabled:
            items.append(self.item_copy_last)
            items.append(None)
        else:
            disabled_copy = rumps.MenuItem(
                "Copy Latest to Clipboard (history off)", callback=lambda _s: None
            )
            disabled_copy.state = 0
            disabled_copy.title = "Copy Latest to Clipboard (history off)"
            items.append(disabled_copy)
            items.append(None)

        items.append(self.item_open)
        set_submenu(self.menu, items)

    # ------------------------------------------------------------------
    def toggle_history(self, _sender=None) -> None:
        app = self.app
        app.history_enabled = not app.history_enabled
        from ...config import update_env_vars  # lazy import to avoid cycles

        update_env_vars({"HISTORY_ENABLED": "1" if app.history_enabled else "0"})
        set_status(app, "History enabled" if app.history_enabled else "History disabled")
        self.refresh()

    def copy_entry(self, path: Path) -> None:
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except Exception as exc:
            import logging

            logging.getLogger(__name__).error("Failed to read history entry %s: %s", path, exc)
            return
        copy_text(text)
        try:
            rumps.notification(
                title="VistaScribe",
                subtitle="Copied from history",
                message=path.name,
            )
        except Exception:
            pass

    def copy_latest(self) -> None:
        if not self.app.history_enabled:
            return
        entries = history.recent_entries(1)
        if not entries:
            return
        self.copy_entry(entries[0].path)

    def archive_transcript(self, text: str) -> None:
        copy_text(text)
        if not self.app.history_enabled:
            self._latest_history_path = None
            set_status(self.app, "Copied to clipboard (history off)")
            try:
                rumps.notification(
                    title="VistaScribe",
                    subtitle="History disabled",
                    message="Transcript sent to clipboard",
                )
            except Exception:
                pass
            return

        entry = history.save_entry(text)
        self._latest_history_path = entry.path
        set_status(self.app, "Saved to history (clipboard)")
        try:
            rumps.notification(
                title="VistaScribe",
                subtitle="Saved to history",
                message="No editable field detected",
            )
        except Exception:
            pass
        self.schedule_refresh()
