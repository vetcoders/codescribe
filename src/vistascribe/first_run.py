from __future__ import annotations

import json
import logging
import os

from .onboarding import OnboardingWizard
from .path_utils import user_data_root

logger = logging.getLogger(__name__)

CURRENT_CONFIG_VERSION = 1


APP_SUPPORT_DIR = user_data_root()
CONFIG_JSON = APP_SUPPORT_DIR / "config.json"
DEFAULT_CONFIG: dict[str, object] = {
    "restore_clipboard": True,
    "hold_mods": "ctrl",
    "hold_exclusive": True,
    "beep_on_start": False,
    "config_version": CURRENT_CONFIG_VERSION,
}


def _ensure_dirs() -> None:
    APP_SUPPORT_DIR.mkdir(parents=True, exist_ok=True)


def load_json() -> dict:
    if not CONFIG_JSON.exists():
        return {}
    try:
        return json.loads(CONFIG_JSON.read_text(encoding="utf-8"))
    except Exception as exc:
        logger.warning("Failed to load %s: %s", CONFIG_JSON, exc)
        return {}


def _merge_config(data: dict) -> dict:
    version = data.get("config_version", 0)
    if version and version < CURRENT_CONFIG_VERSION:
        logger.info("Migrating config from version %s to %s", version, CURRENT_CONFIG_VERSION)
    merged = DEFAULT_CONFIG | data
    merged["config_version"] = CURRENT_CONFIG_VERSION
    return merged


def save_json(cfg: dict) -> None:
    _ensure_dirs()
    cfg = _merge_config(cfg)
    CONFIG_JSON.write_text(json.dumps(cfg, indent=2, ensure_ascii=False), encoding="utf-8")


def ensure_config_and_permissions() -> None:
    """Ensure first-run config is applied; trigger onboarding wizard if required."""

    if OnboardingWizard.should_run():
        wizard = OnboardingWizard()
        if not wizard.run():
            logger.info("Onboarding wizard cancelled; will prompt again next launch.")
            return

    cfg = load_json()
    if cfg:
        cfg = _merge_config(cfg)
    else:
        cfg = DEFAULT_CONFIG.copy()
        save_json(cfg)

    if cfg.get("restore_clipboard", True):
        os.environ["RESTORE_CLIPBOARD"] = "1"
    else:
        os.environ["RESTORE_CLIPBOARD"] = "0"

    os.environ["HOLD_MODS"] = str(cfg.get("hold_mods", "ctrl")).lower()
    os.environ["HOLD_EXCLUSIVE"] = "1" if cfg.get("hold_exclusive", True) else "0"
    os.environ["BEEP_ON_START"] = "1" if cfg.get("beep_on_start", False) else "0"
