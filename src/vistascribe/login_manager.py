# login_manager.py - extracted from main.py
import logging
import os
import plistlib

logger = logging.getLogger(__name__)


class LoginManager:
    def _login_plist_path(self) -> str:
        home = os.path.expanduser("~")
        return os.path.join(home, "Library", "LaunchAgents", "com.vistascribe.tray.plist")

    def _is_login_installed(self) -> bool:
        return os.path.exists(self.app._login_plist_path())

    def _toggle_login_item(self, _sender):
        try:
            if self.app._is_login_installed():
                self.app._remove_login_agent()
            else:
                self.app._install_login_agent()
        except Exception as e:
            logger.error(f"Login item toggle failed: {e}")
        try:
            self.app.menu["Start at Login"].state = self._is_login_installed()
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

    def _install_login_agent(self):
        path = self.app._login_plist_path()
        os.makedirs(os.path.dirname(path), exist_ok=True)
        app_repo = "/Applications/VistaScribe.app/Contents/Resources/Repo"
        cmd = (
            f"cd '{app_repo}' && ./scripts/quickstart_mac.sh --mode both --daemon --log "
            f"'$HOME/Library/Logs/VistaScribe.app.log'"
        )
        data = {
            "Label": "com.vistascribe.tray",
            "ProgramArguments": ["/bin/zsh", "-lc", cmd],
            "RunAtLoad": True,
            "KeepAlive": False,
            "StandardOutPath": os.path.expanduser("~/Library/Logs/VistaScribe.launchd.out.log"),
            "StandardErrorPath": os.path.expanduser("~/Library/Logs/VistaScribe.launchd.err.log"),
        }
        with open(path, "wb") as f:
            plistlib.dump(data, f)
        import subprocess

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        subprocess.run(["launchctl", "load", "-w", path], check=False)
        logger.info("Installed Start at Login LaunchAgent")

    def _remove_login_agent(self):
        path = self.app._login_plist_path()
        import subprocess

        subprocess.run(["launchctl", "unload", "-w", path], check=False)
        try:
            os.remove(path)
        except FileNotFoundError:
            logger.debug("LaunchAgent plist already removed: %s", path)
        logger.info("Removed Start at Login LaunchAgent")

    def __init__(self, app):
        self.app = app
