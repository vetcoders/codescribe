"""Models menu controller."""

from __future__ import annotations

import logging
import os
import subprocess
import threading
from collections.abc import Callable
from pathlib import Path

import rumps

from ..menu_utils import create_parent_item, set_submenu

logger = logging.getLogger(__name__)


class ModelsController:
    def __init__(self, app):
        self.app = app
        self.menu = create_parent_item("Models")
        self._download_lock = threading.Lock()
        self.refresh()

    def refresh(self):
        from ... import stt as stt_mod
        from ...menu_model import build_models_spec, render_rumps_menu  # lazy import

        try:
            cur = stt_mod.get_current_variant()
        except Exception:
            cur = "unknown"
        label_map = {
            "small": "Small",
            "medium": "Medium",
            "large-v3": "Large v3",
            "large-v3-turbo": "Large v3 Turbo",
            "remote": "Remote",
        }
        current_label = label_map.get(cur, cur)
        is_remote = bool(os.environ.get("WHISPER_SERVER_URL", "").strip())

        ollama_models = self._get_ollama_models()
        spec = build_models_spec(is_remote, current_label, ollama_models)
        actions: dict[str, Callable[[], None]] = {
            "use_small": lambda: self.set_variant("small"),
            "use_medium": lambda: self.set_variant("medium"),
            "use_lv3": lambda: self.set_variant("large-v3"),
            "use_lvt": lambda: self.set_variant("large-v3-turbo"),
            "open_models": self._open_models_folder,
        }
        for model in ollama_models:
            actions[f"ollama_{model}"] = lambda m=model: self.app._set_ollama_model(m)

        entries = render_rumps_menu(self.app, spec, actions)
        set_submenu(self.menu, entries)

    def refresh_async(self):
        helper = getattr(rumps, "AppHelper", None)
        if helper is not None:
            helper.callAfter(self.refresh)
        else:
            self.refresh()

    def set_variant(self, variant: str):
        if os.environ.get("WHISPER_SERVER_URL", "").strip():
            rumps.alert(
                title="Remote Whisper active",
                message="Disable WHISPER_SERVER_URL to switch local models.",
            )
            return
        from ... import stt as stt_mod

        desired_path = stt_mod.find_variant_path(variant)
        if desired_path is None:
            self._prompt_download(variant)
            return

        if not stt_mod.set_variant(variant):
            rumps.alert(
                title="Model not found",
                message="Download the model first using the helper script.",
            )
            return

        try:
            wd = os.environ.get("WHISPER_DIR") or ""
            self.app._persist_env_vars(
                {
                    "WHISPER_VARIANT": variant,
                    "WHISPER_DIR": wd,
                }
            )
        except Exception:
            pass
        logger.info("✓ Model switched to: %s", variant)
        self.refresh_async()
        try:
            rumps.notification(
                title="Whisper model",
                subtitle="Switched",
                message=variant.replace("-", " "),
            )
        except Exception:
            pass

    def _prompt_download(self, variant: str):
        try:
            window = rumps.Window(
                message=(
                    f"Whisper '{variant}' is not installed.\n"
                    "Download now using scripts/get_models.py?"
                ),
                title="Download Whisper Model",
                ok="Download",
                cancel="Cancel",
            )
            resp = window.run()
            if not resp.clicked:
                return
        except Exception:
            return
        threading.Thread(
            target=self._download_worker,
            args=(variant,),
            daemon=True,
        ).start()

    def _download_worker(self, variant: str):
        script = Path(self.app.repo_root) / "scripts" / "get_models.py"
        if not script.exists():
            try:
                rumps.alert(
                    title="Missing script",
                    message="scripts/get_models.py not found. Download manually.",
                )
            except Exception:
                pass
            return
        cmd = ["uv", "run", "python", str(script), "--whisper", variant]
        try:
            result = subprocess.run(
                cmd,
                cwd=self.app.repo_root,
                capture_output=True,
                text=True,
                check=False,
            )
            if result.returncode != 0:
                logger.error("Model download failed: %s", result.stderr.strip())
                try:
                    rumps.alert(
                        title="Download failed",
                        message=result.stderr.strip() or "Model fetch failed.",
                    )
                except Exception:
                    pass
                return
            logger.info("Model '%s' downloaded", variant)
            from ... import stt as stt_mod

            if stt_mod.set_variant(variant):
                self.refresh_async()
            try:
                rumps.notification(
                    title="Whisper download",
                    subtitle="Completed",
                    message=f"Variant: {variant}",
                )
            except Exception:
                pass
        except Exception as exc:
            logger.error("Download thread crashed: %s", exc)
            try:
                rumps.alert(title="Download error", message=str(exc))
            except Exception:
                pass

    def _open_models_folder(self, _sender=None):
        models_dir = os.path.join(self.app.repo_root, "models")
        os.makedirs(models_dir, exist_ok=True)
        try:
            subprocess.run(["open", models_dir])
        except Exception:
            rumps.alert(title="Open Folder", message=models_dir)

    def _get_ollama_models(self) -> list[str]:
        ollama_host = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434").strip()
        try:
            import requests

            r = requests.get(ollama_host.rstrip("/") + "/api/tags", timeout=2)
            if r.status_code == 200:
                data = r.json()
                models = [item.get("name") for item in data.get("models", [])]
                return [m for m in models if m]
        except Exception as exc:
            logger.debug("Ollama list failed: %s", exc)
        return []
