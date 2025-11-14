"""Onboarding wizard for VistaScribe first-run experience."""

from __future__ import annotations

import os
import shutil
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path

from huggingface_hub import snapshot_download

try:  # pragma: no cover - UI code exercised on macOS only
    import AppKit  # type: ignore
except Exception:  # pragma: no cover
    AppKit = None  # type: ignore

from ..config import update_env_vars
from ..path_utils import repo_root
from ..settings_store import get_settings, save_settings

MODELS_DIR = repo_root() / "models"
WHISPER_REPOS = {
    "tiny": "mlx-community/whisper-tiny-mlx",
    "base": "mlx-community/whisper-base-mlx",
    "small": "mlx-community/whisper-small-mlx",
    "medium": "mlx-community/whisper-medium-mlx",
    "large-v3": "mlx-community/whisper-large-v3-mlx",
    "large-v3-turbo": "mlx-community/whisper-large-v3-turbo",
}

MODEL_ORDER = ["tiny", "base", "small", "medium", "large-v3"]
MODEL_LABELS = {
    "tiny": "Tiny",
    "base": "Base",
    "small": "Small",
    "medium": "Medium",
    "large-v3": "Large v3",
}


@dataclass
class WizardResult:
    variant: str
    model_path: Path
    ai_enabled: bool
    ai_provider: str
    ai_url: str | None
    ai_api_key: str | None


class OnboardingWizard:
    """Simple multi-step wizard to bootstrap first-run installs."""

    def __init__(self) -> None:
        self._hf_token = os.environ.get("HF_TOKEN")
        self._result: WizardResult | None = None
        self._bundled = "Resources" in str(repo_root())

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------
    @classmethod
    def should_run(cls) -> bool:
        env_missing = not (repo_root() / ".env").exists()
        settings_missing = False
        try:
            from ..settings_store import _settings_path  # type: ignore

            settings_missing = not _settings_path().exists()
        except Exception:  # pragma: no cover - defensive
            settings_missing = True
        return env_missing or settings_missing

    def run(self) -> bool:
        if AppKit is None:
            return self._run_cli()
        return self._run_gui()

    # ------------------------------------------------------------------
    # UI utilities
    # ------------------------------------------------------------------
    def _focus_app(self) -> None:
        if AppKit is None:
            return
        try:
            app = AppKit.NSApplication.sharedApplication()
            if app is not None:
                app.activateIgnoringOtherApps_(True)
        except Exception:
            pass
        try:
            runner = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(
                os.getpid()
            )
            if runner is not None:
                runner.activateWithOptions_(AppKit.NSApplicationActivateIgnoringOtherApps)
        except Exception:
            pass

    def _run_alert(self, alert):  # pragma: no cover - UI only
        self._focus_app()
        try:
            window = alert.window()
            if window is not None:
                window.makeKeyAndOrderFront_(None)
                window.setLevel_(AppKit.NSFloatingWindowLevel)
                self._position_alert_window(window)
        except Exception:
            pass
        return alert.runModal()

    def _show_progress_panel(self, message: str, done: threading.Event):  # pragma: no cover
        if AppKit is None:
            done.wait()
            return

        indicator = AppKit.NSProgressIndicator.alloc().initWithFrame_(((134, 36), (32, 32)))
        indicator.setStyle_(AppKit.NSProgressIndicatorSpinningStyle)
        indicator.setDisplayedWhenStopped_(False)
        indicator.startAnimation_(None)

        label = AppKit.NSTextField.alloc().initWithFrame_(((20, 80), (260, 20)))
        label.setBezeled_(False)
        label.setDrawsBackground_(False)
        label.setEditable_(False)
        label.setSelectable_(False)
        label.setStringValue_(message)

        content = AppKit.NSView.alloc().initWithFrame_(((0, 0), (300, 130)))
        content.addSubview_(indicator)
        content.addSubview_(label)

        mask = AppKit.NSWindowStyleMaskTitled | AppKit.NSWindowStyleMaskClosable
        panel = AppKit.NSPanel.alloc().initWithContentRect_styleMask_backing_defer_(
            ((0, 0), (300, 130)), mask, AppKit.NSBackingStoreBuffered, False
        )
        panel.setTitle_("Downloading model…")
        panel.setContentView_(content)
        panel.setLevel_(AppKit.NSFloatingWindowLevel)
        panel.setReleasedWhenClosed_(True)

        self._focus_app()
        panel.makeKeyAndOrderFront_(None)
        self._position_alert_window(panel)

        run_loop = AppKit.NSRunLoop.currentRunLoop()
        while not done.wait(0.1):
            run_loop.runUntilDate_(AppKit.NSDate.dateWithTimeIntervalSinceNow_(0.05))

        indicator.stopAnimation_(None)
        panel.orderOut_(None)

    def _position_alert_window(self, window):  # pragma: no cover - UI only
        try:
            screen = AppKit.NSScreen.mainScreen()
            if screen is None:
                window.center()
                return
            screen_frame = screen.visibleFrame()
            win_frame = window.frame()
            new_x = screen_frame.origin.x + (screen_frame.size.width - win_frame.size.width) / 2
            new_y = screen_frame.origin.y + (screen_frame.size.height - win_frame.size.height) / 2
            window.setFrameOrigin_((new_x, new_y))
        except Exception:
            try:
                window.center()
            except Exception:
                pass

    # ------------------------------------------------------------------
    # CLI fallback
    # ------------------------------------------------------------------
    def _run_cli(self) -> bool:
        installer = self._build_installer_state()
        options = ", ".join(MODEL_ORDER)
        variant = input(f"Select Whisper variant [{options}] (default medium): ").strip()
        if variant not in WHISPER_REPOS:
            variant = "medium"
        force_download = False
        if installer[variant]["installed"]:
            force_download = self._confirm_redownload_cli(variant)
        model_path = self._download_variant(variant, force_download=force_download)
        ai_choice = input("Enable AI formatting via Ollama? [y/N]: ").strip().lower()
        ai_enabled = ai_choice == "y"
        provider = "ollama" if ai_enabled else "harmony"
        self._result = WizardResult(
            variant=variant,
            model_path=model_path,
            ai_enabled=ai_enabled,
            ai_provider=provider,
            ai_url="http://127.0.0.1:11434" if provider == "ollama" and ai_enabled else None,
            ai_api_key=None,
        )
        self._persist()
        return True

    # ------------------------------------------------------------------
    # macOS Cocoa UI
    # ------------------------------------------------------------------
    def _run_gui(self) -> bool:  # pragma: no cover - UI tested manually
        if not self._step_intro():
            return False
        variant = self._step_model()
        if not variant:
            return False
        force_download = False
        if self._variant_installed(variant):
            redl = self._confirm_redownload(variant)
            if redl is None:
                return False
            force_download = redl
        model_path = self._download_variant(variant, force_download=force_download)
        self._step_permissions()
        ai_enabled, provider, url, key = self._step_ai()
        self._result = WizardResult(
            variant=variant,
            model_path=model_path,
            ai_enabled=ai_enabled,
            ai_provider=provider,
            ai_url=url,
            ai_api_key=key,
        )
        self._persist()
        return True

    def _nsalert(self, title: str, message: str, buttons: list[str]) -> int:
        alert = AppKit.NSAlert.new()
        alert.setMessageText_(title)
        alert.setInformativeText_(message)
        for title in buttons:
            alert.addButtonWithTitle_(title)
        return self._run_alert(alert)

    def _step_intro(self) -> bool:
        resp = self._nsalert(
            "VistaScribe setup",
            "This is the first run on this machine. Let's download a Whisper model, "
            "configure permissions, and optionally enable AI formatting.",
            ["Next", "Quit"],
        )
        return resp == AppKit.NSAlertFirstButtonReturn

    def _step_model(self) -> str | None:
        popup = AppKit.NSPopUpButton.alloc().initWithFrame_pullsDown_(((0, 0), (320, 26)), False)
        installer = self._build_installer_state()
        ordered = MODEL_ORDER
        for variant in ordered:
            popup.addItemWithTitle_(installer[variant]["label"])
        accessory = AppKit.NSView.alloc().initWithFrame_(((0, 0), (280, 34)))
        popup.setFrameOrigin_((0, 4))
        accessory.addSubview_(popup)
        alert = AppKit.NSAlert.new()
        alert.setMessageText_("Choose Whisper variant")
        alert.setInformativeText_(
            "Large models are more accurate but need more RAM. You can change this later."
        )
        alert.setAccessoryView_(accessory)
        alert.addButtonWithTitle_("Download")
        alert.addButtonWithTitle_("Quit")
        resp = self._run_alert(alert)
        if resp != AppKit.NSAlertFirstButtonReturn:
            return None
        idx = popup.indexOfSelectedItem()
        try:
            return ordered[idx]
        except Exception:
            return "medium"

    def _model_dir(self, variant: str) -> Path:
        return MODELS_DIR / f"whisper-{variant}"

    def _build_installer_state(self) -> dict[str, dict[str, object]]:
        state: dict[str, dict[str, object]] = {}
        for variant in MODEL_ORDER:
            installed = self._variant_installed(variant)
            icon = "✔" if installed else "↓"
            label = f"{icon} {MODEL_LABELS.get(variant, variant)}"
            state[variant] = {"installed": installed, "label": label}
        return state

    def _variant_installed(self, variant: str) -> bool:
        target = self._model_dir(variant)
        return target.exists() and any(target.iterdir())

    def _confirm_redownload(self, variant: str) -> bool | None:
        resp = self._nsalert(
            "Model already present",
            f"Whisper '{variant}' is already installed. Redownload it?",
            ["Redownload", "Use existing", "Cancel"],
        )
        if resp == AppKit.NSAlertFirstButtonReturn:
            return True
        if resp == AppKit.NSAlertSecondButtonReturn:
            return False
        return None

    def _confirm_redownload_cli(self, variant: str) -> bool:
        ans = input(f"Model '{variant}' already exists. Redownload? [y/N]: ").strip().lower()
        return ans.startswith("y")

    def _download_variant(self, variant: str, *, force_download: bool = False) -> Path:
        if AppKit is None:
            return self._download_variant_sync(variant, force_download=force_download)
        return self._download_variant_with_ui(variant, force_download=force_download)

    def _download_variant_with_ui(self, variant: str, *, force_download: bool) -> Path:
        result: dict[str, Path | Exception | None] = {"path": None, "error": None}
        done = threading.Event()

        def worker():
            try:
                result["path"] = self._download_variant_sync(variant, force_download=force_download)
            except Exception as exc:  # pragma: no cover - UI path only
                result["error"] = exc
            finally:
                done.set()

        threading.Thread(target=worker, daemon=True).start()
        self._show_progress_panel(f"Downloading Whisper '{variant}'…", done)
        done.wait()
        if isinstance(result.get("error"), Exception):
            raise result["error"]  # type: ignore[misc]
        return result["path"]  # type: ignore[return-value]

    def _download_variant_sync(self, variant: str, *, force_download: bool = False) -> Path:
        MODELS_DIR.mkdir(parents=True, exist_ok=True)
        target = self._model_dir(variant)
        if target.exists():
            if not force_download and any(target.iterdir()):
                return target
            if force_download:
                shutil.rmtree(target, ignore_errors=True)
                target.mkdir(parents=True, exist_ok=True)

        while True:
            try:
                snapshot_download(
                    repo_id=WHISPER_REPOS[variant],
                    local_dir=str(target),
                    local_dir_use_symlinks=False,
                    token=self._hf_token,
                    resume_download=True,
                )
                break
            except Exception as exc:
                if not self._prompt_hf_token(str(exc)):
                    raise
        return target

    def _prompt_hf_token(self, error_msg: str) -> bool:
        if AppKit is None:
            token = input("Enter Hugging Face token (blank to abort): ").strip()
            if not token:
                return False
            self._hf_token = token
            os.environ["HF_TOKEN"] = token
            return True
        alert = AppKit.NSAlert.new()
        alert.setMessageText_("Hugging Face token required")
        alert.setInformativeText_(f"{error_msg}\nEnter your HF token to retry.")
        field = AppKit.NSTextField.alloc().initWithFrame_(((0, 0), (280, 24)))
        field.setStringValue_(self._hf_token or "")
        accessory = AppKit.NSView.alloc().initWithFrame_(((0, 0), (300, 36)))
        field.setFrameOrigin_((0, 4))
        accessory.addSubview_(field)
        alert.setAccessoryView_(accessory)
        alert.addButtonWithTitle_("Retry")
        alert.addButtonWithTitle_("Cancel")
        resp = self._run_alert(alert)
        if resp != AppKit.NSAlertFirstButtonReturn:
            return False
        token = field.stringValue().strip()
        if not token:
            return False
        self._hf_token = token
        os.environ["HF_TOKEN"] = token
        return True

    def _step_permissions(self) -> None:
        resp = self._nsalert(
            "Permissions",
            "VistaScribe needs Accessibility, Input Monitoring, and Microphone permissions. "
            "You can grant them now or adjust later from the Permissions menu.",
            ["Open Settings", "Skip"],
        )
        if resp == AppKit.NSAlertFirstButtonReturn:
            self._open_privacy_settings()
            self._nsalert(
                "Grant permissions",
                "System Settings should now highlight Accessibility, Input Monitoring, and "
                "Microphone. Enable VistaScribe (or your preferred terminal) in each list, then "
                "click Continue.",
                ["Continue"],
            )

    def _open_privacy_settings(self) -> None:
        try:
            script = """
tell application "System Settings"
  activate
  delay 0.2
  reveal anchor "Privacy_Accessibility" of pane id "com.apple.settings.PrivacySecurity"
  delay 0.4
  reveal anchor "Privacy_ListenEvent" of pane id "com.apple.settings.PrivacySecurity"
  delay 0.4
  reveal anchor "Privacy_Microphone" of pane id "com.apple.settings.PrivacySecurity"
end tell
"""
            subprocess.run(["osascript", "-e", script], check=False)
        except Exception:
            pass

    def _step_ai(self) -> tuple[bool, str, str | None, str | None]:
        resp = self._nsalert(
            "AI formatting (optional)",
            "Light+ cleanup runs locally. Enable AI formatting only if you're comfortable "
            "sending transcripts to Ollama or a Harmony-compatible API.",
            ["Enable", "Skip"],
        )
        if resp != AppKit.NSAlertFirstButtonReturn:
            return False, "harmony", None, None

        popup = AppKit.NSPopUpButton.alloc().initWithFrame_pullsDown_(((0, 0), (220, 26)), False)
        popup.addItemWithTitle_("Ollama (local)")
        popup.addItemWithTitle_("Harmony/OpenAI compatible API")
        url_field = AppKit.NSTextField.alloc().initWithFrame_(((0, 0), (300, 24)))
        url_field.setPlaceholderString_("Server URL")
        key_field = AppKit.NSSecureTextField.alloc().initWithFrame_(((0, 0), (300, 24)))
        key_field.setPlaceholderString_("API key (optional)")
        accessory = AppKit.NSView.alloc().initWithFrame_(((0, 0), (320, 80)))
        popup.setFrameOrigin_((0, 50))
        url_field.setFrameOrigin_((0, 26))
        key_field.setFrameOrigin_((0, 0))
        accessory.addSubview_(popup)
        accessory.addSubview_(url_field)
        accessory.addSubview_(key_field)
        alert = AppKit.NSAlert.new()
        alert.setMessageText_("Formatter details")
        alert.setInformativeText_("Provide endpoint URL and API key (if required).")
        alert.setAccessoryView_(accessory)
        alert.addButtonWithTitle_("Save")
        alert.addButtonWithTitle_("Skip")
        resp = self._run_alert(alert)
        if resp != AppKit.NSAlertFirstButtonReturn:
            return False, "harmony", None, None
        provider = "ollama" if popup.indexOfSelectedItem() == 0 else "harmony"
        url = url_field.stringValue().strip() or None
        api_key = key_field.stringValue().strip() or None
        if provider == "ollama" and not url:
            url = "http://127.0.0.1:11434"
        return True, provider, url, api_key

    # ------------------------------------------------------------------
    # Persistence
    # ------------------------------------------------------------------
    def _persist(self) -> None:
        if not self._result:
            return
        updates = {
            "WHISPER_VARIANT": self._result.variant,
            "WHISPER_DIR": str(self._result.model_path),
        }
        if self._result.ai_url:
            url = (self._result.ai_url or "").strip()
            if self._result.ai_provider == "ollama":
                updates["OLLAMA_HOST"] = url
            else:
                if url.lower().endswith("/responses"):
                    url = url[: -len("/responses")]
                updates["HARMONY_BASE_URL"] = url
        if self._result.ai_api_key and not self._bundled:
            updates["HARMONY_API_KEY"] = self._result.ai_api_key
        update_env_vars(updates)

        settings = get_settings()
        settings.ai_formatting_enabled = self._result.ai_enabled
        settings.ai_provider = self._result.ai_provider
        save_settings(settings)
        if self._result.ai_api_key and self._bundled:
            self._store_keychain_secret("VistaScribeAIKey", self._result.ai_api_key)

    def _store_keychain_secret(self, label: str, secret: str) -> None:
        try:
            subprocess.run(
                [
                    "security",
                    "add-generic-password",
                    "-a",
                    "VistaScribe",
                    "-s",
                    label,
                    "-w",
                    secret,
                    "-U",
                ],
                check=True,
            )
        except Exception:
            pass


__all__ = ["OnboardingWizard"]
