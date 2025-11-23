"""Mix-in for developer tool helpers (chat demo, tester, logs)."""

from __future__ import annotations

import logging
import os
import shlex
import subprocess
import sys
import threading

import requests
import rumps

from ...llm import _harmony_base_url
from ...path_utils import repo_root
from ...settings_store import get_settings
from ..menu_utils import create_parent_item, set_submenu

logger = logging.getLogger(__name__)


def _backend_host() -> str:
    host = os.environ.get("VISTASCRIBE_HOST", "127.0.0.1").strip()
    return host or "127.0.0.1"


class ToolsMixin:
    def _init_tools_menu(self):
        self.item_open_lab = rumps.MenuItem(
            "Voice & Chat Lab (Browser)", callback=self._open_voice_chat_lab
        )
        self.item_chat_demo = rumps.MenuItem("AI Chat Demo", callback=self._open_chat_demo)
        self.item_export_menu = rumps.MenuItem("Export Menu Tree…", callback=self._export_menu_tree)
        self.item_open_logs = rumps.MenuItem("Open Logs Folder", callback=self._open_logs_folder)
        self.menu_tools = create_parent_item("Tools")
        set_submenu(self.menu_tools, self._tools_menu_entries())
        self._refresh_chat_demo_label()

    def _tools_menu_entries(self) -> list[rumps.MenuItem | None]:
        return [
            self.item_open_lab,
            self.item_chat_demo,
            None,
            self.item_export_menu,
            self.item_open_logs,
        ]

    def _rebuild_tools_menu(self) -> None:
        set_submenu(self.menu_tools, self._tools_menu_entries())
        self._refresh_chat_demo_label()

    def _refresh_chat_demo_label(self) -> None:
        try:
            provider = get_settings(force_reload=True).ai_provider
        except Exception:
            provider = "harmony"
        if provider not in {"harmony", "ollama"}:
            provider = "harmony"
        label = "AI Chat Demo (Harmony)" if provider == "harmony" else "AI Chat Demo (Ollama)"
        self.item_chat_demo.title = label

    def _chat_demo_env(self) -> dict[str, str]:
        env = os.environ.copy()
        env.setdefault("PYTHONUNBUFFERED", "1")
        return env

    def _open_chat_demo(self, _sender):
        try:
            try:
                settings = get_settings(force_reload=True)
            except Exception:
                settings = None
            provider = getattr(settings, "ai_provider", "harmony") if settings else "harmony"
            python_exe = sys.executable or "python3"
            repo = str(repo_root())

            if provider == "ollama":
                host = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434").strip().rstrip("/")
                model = (
                    os.environ.get("OLLAMA_MODEL", "qwen2.5:3b-instruct").strip()
                    or "qwen2.5:3b-instruct"
                )
                base_arg = shlex.quote(host + "/v1")
                api_key = "ollama"
            else:
                try:
                    base = _harmony_base_url()
                except Exception as exc:
                    rumps.alert(title="AI Chat Demo", message=f"Harmony base URL missing: {exc}")
                    return
                api_key = (
                    os.environ.get("HARMONY_API_KEY")
                    or os.environ.get("LIBRAXIS_API_KEY")
                    or os.environ.get("OPENAI_API_KEY")
                )
                if not api_key:
                    rumps.alert(
                        title="AI Chat Demo",
                        message=(
                            "Set HARMONY_API_KEY (or OPENAI_API_KEY) to use the Harmony chat demo."
                        ),
                    )
                    return
                model = (
                    os.environ.get("HARMONY_CHAT_MODEL")
                    or os.environ.get("HARMONY_MODEL")
                    or "gpt-4o-mini"
                )
                base_clean = base.rstrip("/")
                if base_clean.lower().endswith("/v1"):
                    base_with_v1 = base_clean
                else:
                    base_with_v1 = base_clean + "/v1"
                base_arg = shlex.quote(base_with_v1)
            cmd = (
                f"cd {shlex.quote(repo)} && "
                f"{shlex.quote(python_exe)} -m vistascribe.chatclient "
                f"--base-url {base_arg} "
                f"--api-key {shlex.quote(api_key)} --model {shlex.quote(model)}"
            )
            apple_script = f"""
                tell application "Terminal"
                    activate
                    do script "{cmd}"
                end tell
                """
            threading.Thread(
                target=lambda: subprocess.run(
                    ["osascript", "-e", apple_script], env=self._chat_demo_env()
                ),
                daemon=True,
            ).start()
        except Exception as exc:
            try:
                rumps.alert(title="AI Chat Demo", message=f"Failed to launch chat demo: {exc}")
            except Exception as exc:
                logger.debug("Suppressed exception", exc_info=exc)

    def _open_voice_chat_lab(self, _sender):
        def _worker():
            try:
                host = _backend_host()
                port_file = os.path.join(repo_root(), "logs", "vistascribe-server.port")
                candidates: list[int] = []
                if os.path.exists(port_file):
                    with open(port_file, encoding="utf-8") as handle:
                        val = int((handle.read() or "8237").strip())
                        if val not in candidates:
                            candidates.append(val)
                for p in (8237, 7237, 6237, 5237):
                    if p not in candidates:
                        candidates.append(p)

                def _make_url(port: int) -> str:
                    safe_host = host
                    if ":" in safe_host and not safe_host.startswith("["):
                        safe_host = f"[{safe_host}]"
                    return f"http://{safe_host}:{port}"

                def _is_backend(port: int) -> bool:
                    try:
                        resp = requests.get(f"{_make_url(port)}/version", timeout=0.75)
                        if resp.ok:
                            data = resp.json()
                            return isinstance(data, dict) and "state" in data and "mlx" in data
                    except Exception:
                        return False
                    return False

                chosen = next((port for port in candidates if _is_backend(port)), None)
                if chosen is None:
                    rumps.AppHelper.callAfter(
                        rumps.alert,
                        title="Voice & Chat Lab",
                        message=(
                            "Could not find the VistaScribe backend. Ensure it is running "
                            "(./VistaScribe start both) and that port 8237 is not taken."
                        ),
                    )
                    return
                subprocess.run(["open", f"{_make_url(chosen)}/tester"])
            except Exception as exc:
                try:
                    rumps.AppHelper.callAfter(
                        rumps.alert, title="Voice & Chat Lab", message=str(exc)
                    )
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)

        threading.Thread(target=_worker, daemon=True).start()

    def _open_logs_folder(self, _sender):
        try:
            logs_dir = os.path.join(repo_root(), "logs")
            os.makedirs(logs_dir, exist_ok=True)
            subprocess.run(["open", logs_dir])
        except Exception:
            try:
                rumps.alert(title="Open Logs Folder", message=logs_dir)
            except Exception as exc:
                logger.debug("Suppressed exception", exc_info=exc)

    def _export_menu_tree(self, _sender):
        try:
            docs_dir = os.path.join(repo_root(), "docs")
            os.makedirs(docs_dir, exist_ok=True)

            def _children(node):
                try:
                    if isinstance(node, rumps.MenuItem):
                        sub = getattr(node, "_menu", None)
                        if sub is None:
                            return []
                        return [sub[k] for k in list(sub.keys())]
                    return [node[k] for k in list(node.keys())]
                except Exception:
                    return []

            def _label(node):
                try:
                    if isinstance(node, rumps.MenuItem):
                        return node.title
                except Exception as exc:
                    logger.debug("Suppressed exception", exc_info=exc)
                return str(node)

            md_lines = ["# VistaScribe Menu Tree", ""]

            def walk(node, prefix=""):
                for child in _children(node):
                    title = _label(child) or "(separator)"
                    md_lines.append(f"{prefix}- {title}")
                    walk(child, prefix + "  ")

            walk(self.menu)
            md_path = os.path.join(docs_dir, "menu_tree.md")
            with open(md_path, "w", encoding="utf-8") as handle:
                handle.write("\n".join(md_lines) + "\n")

            mmd_lines = ["```mermaid", "mindmap", "  root((VistaScribe))"]

            def walk_mmd(node, indent="  "):
                for child in _children(node):
                    label = (_label(child) or "(separator)").replace("`", "'")
                    safe = label.replace("::", "|")
                    mmd_lines.append(f"{indent}  {safe}")
                    walk_mmd(child, indent + "  ")

            walk_mmd(self.menu)
            mmd_lines.append("```")
            mmd_path = os.path.join(docs_dir, "menu_tree.mmd")
            with open(mmd_path, "w", encoding="utf-8") as handle:
                handle.write("\n".join(mmd_lines) + "\n")

            rumps.notification(title="VistaScribe", subtitle="Menu exported", message=md_path)
        except Exception as exc:
            import logging

            logging.getLogger(__name__).error("Failed to export menu: %s", exc, exc_info=True)
            try:
                rumps.alert(title="VistaScribe", message=f"Failed to export menu: {exc}")
            except Exception as exc:
                logger.debug("Suppressed exception", exc_info=exc)
