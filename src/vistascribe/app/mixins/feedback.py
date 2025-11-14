"""Mix-in for feedback/start-sound menu handling."""

from __future__ import annotations

import logging
import os

import rumps

from ...config import update_env_vars
from ..menu_utils import create_parent_item, set_submenu


class FeedbackMenuMixin:
    def _init_feedback_menu(self):
        self.item_beep = rumps.MenuItem("Enable Start Sound", callback=self._toggle_beep)
        self.item_sound_tink = rumps.MenuItem(
            "Sound: Tink", callback=lambda _s: self._set_sound_name("Tink")
        )
        self.item_sound_pop = rumps.MenuItem(
            "Sound: Pop", callback=lambda _s: self._set_sound_name("Pop")
        )
        self.item_volume = rumps.MenuItem("Set Volume…", callback=self._set_sound_volume)
        self.item_sound_save = rumps.MenuItem(
            "Save Feedback to .env", callback=self._save_feedback_env
        )
        self.menu_feedback = create_parent_item("Feedback")
        set_submenu(self.menu_feedback, self._feedback_menu_entries())
        self._refresh_feedback_menu()

    def _feedback_menu_entries(self) -> list[rumps.MenuItem | None]:
        return [
            self.item_beep,
            None,
            self.item_sound_tink,
            self.item_sound_pop,
            self.item_volume,
            None,
            self.item_sound_save,
        ]

    def _rebuild_feedback_menu(self) -> None:
        set_submenu(self.menu_feedback, self._feedback_menu_entries())

    def _refresh_feedback_menu(self):
        self.item_beep.state = int(getattr(self, "beep_on_start", True))
        current = os.environ.get("SOUND_NAME", "Tink")
        self.item_sound_tink.state = current == "Tink"
        self.item_sound_pop.state = current == "Pop"

    def _toggle_beep(self, _sender):
        self.beep_on_start = not getattr(self, "beep_on_start", True)
        try:
            update_env_vars({"BEEP_ON_START": "1" if self.beep_on_start else "0"})
        except Exception:
            pass
        self._refresh_feedback_menu()

    def _set_sound_name(self, name: str):
        os.environ["SOUND_NAME"] = name
        try:
            update_env_vars({"SOUND_NAME": name})
        except Exception:
            pass
        self._refresh_feedback_menu()

    def _set_sound_volume(self, _sender):
        vol_str = os.environ.get("SOUND_VOLUME", "0.2")
        window = rumps.Window(
            message="Enter start sound volume (0.0 – 1.0)",
            default_text=vol_str,
            title="Start Sound Volume",
            ok="Save",
            cancel="Cancel",
        )
        resp = window.run()
        if resp.clicked:
            try:
                value = max(0.0, min(1.0, float(resp.text.strip())))
                os.environ["SOUND_VOLUME"] = str(value)
                try:
                    update_env_vars({"SOUND_VOLUME": str(value)})
                except Exception:
                    pass
            except Exception:
                rumps.alert(
                    title="Invalid value",
                    message="Please enter a number between 0.0 and 1.0",
                )

    def _save_feedback_env(self, _sender):
        try:
            update_env_vars(
                {
                    "BEEP_ON_START": "1" if getattr(self, "beep_on_start", True) else "0",
                    "SOUND_NAME": os.environ.get("SOUND_NAME", "Tink"),
                    "SOUND_VOLUME": os.environ.get("SOUND_VOLUME", "0.2"),
                }
            )
            rumps.notification(
                title="VistaScribe",
                subtitle="Feedback saved",
                message="Sound settings persisted to .env",
            )
        except Exception as exc:
            logging.getLogger(__name__).error("Failed to save feedback to .env: %s", exc)
