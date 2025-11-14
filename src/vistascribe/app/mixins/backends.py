"""Mix-in handling backend/menu configuration helpers."""

from __future__ import annotations

import importlib
import os
from typing import Any

import requests
import rumps

from ... import client
from ...config import Config, load_config, save_config
from ...llm import get_ai_settings, set_ai_formatting_enabled
from ...stt import get_language, set_language
from ...ui import backend_status_labels, config_labels
from ..menu_utils import create_parent_item, set_submenu


class BackendMenuMixin:
    def _init_language_menu(self):
        self.menu_language = create_parent_item("Language")
        self._refresh_language_submenu()

    def _init_backend_menu(self):
        self.cfg: Config = load_config()
        self._reload_settings()
        self.item_stt_status = rumps.MenuItem("STT: OFF")
        self.item_ai_status = rumps.MenuItem("AI: Light+ only")
        self.item_w_url = rumps.MenuItem("Whisper URL: local", callback=self._edit_whisper_url)
        self.item_ai_url = rumps.MenuItem("Harmony URL: local", callback=self._edit_harmony_url)
        self.item_check = rumps.MenuItem("Check Backends", callback=self._check_backends)
        self.menu_backends = create_parent_item("Backends")
        set_submenu(self.menu_backends, self._backend_menu_entries())

    def _backend_menu_entries(self) -> list[rumps.MenuItem | None]:
        return [
            self.item_stt_status,
            self.item_ai_status,
            None,
            self.item_w_url,
            self.item_ai_url,
            self.item_check,
        ]

    def _rebuild_backend_menu(self) -> None:
        set_submenu(self.menu_backends, self._backend_menu_entries())

    def _apply_cfg_env(self) -> None:
        os.environ["WHISPER_SERVER_URL"] = self.cfg.whisper_url or ""
        os.environ["LLM_SERVER_URL"] = self.cfg.llm_url or ""
        os.environ["WHISPER_LANGUAGE"] = self.cfg.language or ""
        try:
            importlib.reload(client)
            if self.cfg.language in (None, "", "auto"):
                set_language(None)
            else:
                set_language(self.cfg.language)
        except Exception as exc:
            import logging

            logging.getLogger(__name__).warning("Reload after config apply failed: %s", exc)

    def _reload_settings(self) -> None:
        self.settings = get_ai_settings()
        self.cfg.format_enabled = self.settings.ai_formatting_enabled
        self.cfg.ai_provider = getattr(self.settings, "ai_provider", "harmony")

    def _update_backend_menu_labels(self) -> None:
        stt_ok = getattr(self, "_stt_ok", False)
        llm_ok = getattr(self, "_llm_ok", False)
        self._reload_settings()
        stt_lbl, llm_lbl = backend_status_labels(stt_ok, llm_ok)
        self.item_stt_status.title = stt_lbl
        self.item_ai_status.title = llm_lbl
        for idx, label in enumerate(config_labels(self.cfg)):
            if idx == 2:
                self.item_w_url.title = label
            elif idx == 3:
                self.item_ai_url.title = label

    def _edit_whisper_url(self, _sender):
        window = rumps.Window(
            message="Enter Whisper Server URL (empty = local)",
            default_text=self.cfg.whisper_url,
            title="Configure Whisper",
            ok="Save",
            cancel="Cancel",
        )
        resp = window.run()
        if resp.clicked:
            self.cfg.whisper_url = (resp.text or "").strip()
            save_config(self.cfg)
            self._apply_cfg_env()
            self._check_backends(None)

    def _edit_harmony_url(self, _sender):
        window = rumps.Window(
            message="Enter Harmony-compatible URL (empty = default)",
            default_text=self.cfg.llm_url,
            title="Configure Harmony Endpoint",
            ok="Save",
            cancel="Cancel",
        )
        resp = window.run()
        if resp.clicked:
            self.cfg.llm_url = (resp.text or "").strip()
            save_config(self.cfg)
            self._apply_cfg_env()
            self._check_backends(None)

    def _check_backends(self, _sender) -> None:
        def _check(url: str) -> bool:
            if not url:
                return False
            try:
                r = requests.get(url.rstrip("/") + "/healthz", timeout=(1.5, 2.0))
                if r.status_code == 200:
                    data: dict[str, Any] = r.json()
                    return bool(data.get("ok"))
            except Exception:
                return False
            return False

        local_status: dict[str, bool] | None = None
        if self.cfg.whisper_url:
            self._stt_ok = _check(self.cfg.whisper_url)
        else:
            local_status = client.check_server_status()
            self._stt_ok = local_status.get("whisper", False)

        self._reload_settings()

        if self.settings.ai_provider == "ollama":
            ollama_host = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434").strip()
            try:
                r = requests.get(ollama_host.rstrip("/") + "/api/tags", timeout=(1.5, 2.0))
                self._llm_ok = r.status_code == 200
            except Exception as exc:
                import logging

                logging.getLogger(__name__).debug("Ollama check failed: %s", exc)
                self._llm_ok = False
        else:
            if self.cfg.llm_url:
                self._llm_ok = _check(self.cfg.llm_url)
            else:
                self._llm_ok = (local_status or {}).get("llm", False)
        self._update_backend_menu_labels()
        try:
            stt = "OK" if getattr(self, "_stt_ok", False) else "OFF"
            llm = "OK" if getattr(self, "_llm_ok", False) else "OFF"
            rumps.notification(
                title="VistaScribe",
                subtitle="Backend check",
                message=f"STT: {stt} • AI: {llm}",
            )
        except Exception:
            pass

    def _toggle_ai_formatting(self, _sender):
        try:
            current = get_ai_settings().ai_formatting_enabled
            set_ai_formatting_enabled(not current)
            self._reload_settings()
            self._refresh_formatting_menu()
            self._update_backend_menu_labels()
        except Exception as exc:
            import logging

            logging.getLogger(__name__).error("Failed to toggle AI formatting: %s", exc)

    def _toggle_formatting(self, _sender):
        self._toggle_ai_formatting(_sender)

    def _set_language_auto(self, _sender):
        self._set_language(None)

    def _set_language_pl(self, _sender):
        self._set_language("pl")

    def _set_language_en(self, _sender):
        self._set_language("en")

    def _set_language(self, code: str | None):
        set_language(code)
        self.cfg.language = code or "auto"
        save_config(self.cfg)
        self._apply_cfg_env()
        self._refresh_language_submenu()

    def _refresh_language_submenu(self):
        current_lang = get_language()

        items = [
            rumps.MenuItem(
                "✓ Auto" if current_lang is None else "  Auto",
                callback=self._set_language_auto,
            ),
            rumps.MenuItem(
                "✓ Polish (PL)" if current_lang == "pl" else "  Polish (PL)",
                callback=self._set_language_pl,
            ),
            rumps.MenuItem(
                "✓ English (EN)" if current_lang == "en" else "  English (EN)",
                callback=self._set_language_en,
            ),
        ]
        set_submenu(self.menu_language, items)
