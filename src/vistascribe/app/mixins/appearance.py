"""Mix-in for tray appearance helpers."""

from __future__ import annotations

import logging
import os

from ...ui import MenuIcon

logger = logging.getLogger(__name__)


class AppearanceMixin:
    def _show_tray_glyph_enabled(self) -> bool:
        return getattr(self, "show_tray_glyph", True)

    def _refresh_appearance_menu(self):
        if hasattr(self, "item_tray_glyph"):
            self.item_tray_glyph.state = int(self._show_tray_glyph_enabled())

    def _refresh_tray_icon(self):
        glyph = MenuIcon.IDLE
        state = self.recording.current_state()
        if state in {"REC_HOLD", "REC_TOGGLE"}:
            glyph = MenuIcon.LISTEN
        elif state == "BUSY":
            glyph = MenuIcon.THINK
        MenuIcon.set(self, glyph)

    def _toggle_tray_glyph(self, _sender):
        new_val = not self._show_tray_glyph_enabled()
        self.show_tray_glyph = new_val
        os.environ["SHOW_TRAY_GLYPH"] = "1" if new_val else "0"
        try:
            from ...config import update_env_vars

            update_env_vars({"SHOW_TRAY_GLYPH": "1" if new_val else "0"})
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)
        self._refresh_appearance_menu()
        self._refresh_tray_icon()
