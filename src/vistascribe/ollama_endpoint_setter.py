"""Simple Ollama endpoint setter - add this to main.py menu."""


def _set_ollama_endpoint(self, _sender):
    """Set custom Ollama endpoint (for Tailscale, remote servers, etc)."""
    import os

    from .config import update_env_vars

    current = os.environ.get("OLLAMA_HOST", "http://127.0.0.1:11434")

    response = rumps.Window(
        title="Set Ollama Endpoint",
        message="Enter Ollama server URL:\nFor local: http://127.0.0.1:11434\nFor Tailscale: http://dragon:11434\nFor remote: http://YOUR_IP:11434",
        default_text=current,
        ok="Set",
        cancel="Cancel",
        dimensions=(320, 24),
    ).run()

    if response.clicked:
        new_endpoint = response.text.strip()
        if new_endpoint:
            os.environ["OLLAMA_HOST"] = new_endpoint
            try:
                update_env_vars({"OLLAMA_HOST": new_endpoint})

                # Test connection
                import requests

                try:
                    resp = requests.get(f"{new_endpoint}/api/tags", timeout=2)
                    if resp.status_code == 200:
                        rumps.notification(
                            title="VistaScribe",
                            subtitle="Ollama endpoint set",
                            message=f"Connected to: {new_endpoint}",
                        )
                    else:
                        rumps.notification(
                            title="VistaScribe",
                            subtitle="Warning",
                            message=f"Endpoint set but not responding: {new_endpoint}",
                        )
                except:
                    rumps.notification(
                        title="VistaScribe",
                        subtitle="Warning",
                        message=f"Can't reach endpoint: {new_endpoint}",
                    )

                logger.info(f"Ollama endpoint set to: {new_endpoint}")
            except Exception as e:
                logger.error(f"Failed to set Ollama endpoint: {e}")


# Add to menu in main.py __init__:
# self.item_ollama_endpoint = rumps.MenuItem(
#     "Set Ollama Endpoint...",
#     callback=self._set_ollama_endpoint
# )
# Add to Formatting submenu or Backends submenu
