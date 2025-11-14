#!/usr/bin/env python3
"""Detect local hardware/network capabilities and suggest Ollama configs."""

from __future__ import annotations

import json
import logging
import os
import platform
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

MODEL_MATRIX: dict[str, dict[str, Any]] = {
    "qwen3-coder:30b": {"ram_required": 64, "quality": "excellent"},
    "qwen3:14b": {"ram_required": 32, "quality": "very good"},
    "qwen3:7b": {"ram_required": 16, "quality": "good"},
    "qwen3:4b": {"ram_required": 8, "quality": "decent"},
}
DRAGON_KEYWORDS = ("dragon", "m3ultra")


@dataclass
class TailscalePeer:
    hostname: str
    ips: list[str] = field(default_factory=list)


@dataclass
class TailscaleStatus:
    connected: bool = False
    peers: list[TailscalePeer] = field(default_factory=list)

    def dragon_peers(self) -> list[TailscalePeer]:
        return [peer for peer in self.peers if _looks_like_dragon(peer.hostname)]


@dataclass
class HardwareProfile:
    ram_gb: float
    installed_models: list[str]
    tailscale: TailscaleStatus


def _looks_like_dragon(hostname: str) -> bool:
    name = (hostname or "").lower()
    return any(keyword in name for keyword in DRAGON_KEYWORDS)


def get_system_ram_gb(default: float = 16.0) -> float:
    """Return total system RAM in GB with multi-platform fallbacks."""

    try:
        system = platform.system().lower()
        if system == "darwin":
            result = subprocess.run(
                ["sysctl", "-n", "hw.memsize"], capture_output=True, text=True, check=True
            )
            return int(result.stdout.strip()) / (1024**3)
        if system == "linux":
            meminfo = Path("/proc/meminfo")
            if meminfo.exists():
                for line in meminfo.read_text().splitlines():
                    if line.startswith("MemTotal:"):
                        kb = int(line.split()[1])
                        return kb / (1024**2)
        # Windows / others fallback to psutil if available
        try:  # pragma: no cover - optional dependency on non-mac platforms
            import psutil  # type: ignore

            return psutil.virtual_memory().total / (1024**3)
        except Exception:
            logger.debug("psutil not available for RAM detection", exc_info=True)
    except Exception:
        logger.warning("RAM detection failed; falling back to %.1f GB", default, exc_info=True)
    return default


def get_available_models() -> tuple[dict[str, dict[str, Any]], list[str]]:
    """Return static model matrix and the locally installed Ollama models."""

    ollama_bin = shutil.which("ollama")
    if not ollama_bin:
        logger.debug("ollama binary not found on PATH")
        return MODEL_MATRIX, []

    try:
        result = subprocess.run([ollama_bin, "list"], capture_output=True, text=True, check=True)
        installed: list[str] = []
        for line in result.stdout.splitlines()[1:]:
            if not line.strip():
                continue
            installed.append(line.split()[0])
        return MODEL_MATRIX, installed
    except Exception:
        logger.warning("Failed to query ollama list", exc_info=True)
        return MODEL_MATRIX, []


def detect_tailscale() -> TailscaleStatus:
    """Return Tailscale connectivity + peer metadata."""

    tailscale_bin = shutil.which("tailscale")
    if not tailscale_bin:
        logger.debug("tailscale binary not found on PATH")
        return TailscaleStatus()

    try:
        result = subprocess.run(
            [tailscale_bin, "status", "--json"], capture_output=True, text=True, check=True
        )
        status_json = json.loads(result.stdout or "{}")
    except Exception:
        logger.warning("tailscale status --json failed", exc_info=True)
        return TailscaleStatus()

    connected = status_json.get("BackendState") == "Running"
    peers: list[TailscalePeer] = []
    for peer_info in (status_json.get("Peer") or {}).values():
        hostname = peer_info.get("HostName", "")
        ips = peer_info.get("TailscaleIPs") or []
        peers.append(TailscalePeer(hostname=hostname, ips=ips))
    return TailscaleStatus(connected=connected, peers=peers)


def build_hardware_profile() -> HardwareProfile:
    _, installed = get_available_models()
    return HardwareProfile(
        ram_gb=get_system_ram_gb(),
        installed_models=installed,
        tailscale=detect_tailscale(),
    )


def suggest_configuration(profile: HardwareProfile) -> tuple[dict[str, Any], list[str]]:
    """Return (config, summary_lines) for the detected hardware profile."""

    ram_gb = profile.ram_gb
    tailscale = profile.tailscale
    dragon_peers = tailscale.dragon_peers()
    notes = [
        f"Detected RAM: {ram_gb:.1f} GB",
        f"Tailscale: {'connected' if tailscale.connected else 'not connected'}",
        "Installed Ollama models: " + (", ".join(profile.installed_models) or "none"),
    ]
    if dragon_peers:
        peer_labels = [peer.hostname or (peer.ips[0] if peer.ips else "?") for peer in dragon_peers]
        notes.append("Dragon peers: " + ", ".join(peer_labels))

    config: dict[str, Any]
    if ram_gb >= 256:
        config = {
            "mode": "dragon",
            "ollama_host": "http://127.0.0.1:11434",
            "ollama_model": "qwen3-coder:30b",
            "can_serve_team": True,
            "max_new_tokens": 8192,
        }
        notes.append("Mode: Dragon (run any model locally)")
        return config, notes

    if dragon_peers:
        peer = dragon_peers[0]
        host = peer.hostname or (peer.ips[0] if peer.ips else "dragon")
        sanitized_host = host
        config = {
            "mode": "team",
            "ollama_host": f"http://{sanitized_host}:11434",
            "ollama_model": "qwen3-coder:30b",
            "local_fallback": _suggest_local_model(ram_gb),
            "dragon_host": host,
            "max_new_tokens": 4096,
        }
        notes.append(f"Mode: Team (proxy Dragon via {host})")
        return config, notes

    suggested = _suggest_local_model(ram_gb)
    config = {
        "mode": "local",
        "ollama_host": "http://127.0.0.1:11434",
        "ollama_model": suggested,
        "ram_warning": ram_gb < 32,
        "max_new_tokens": _max_tokens_for_ram(ram_gb),
    }
    notes.append(f"Mode: Local (suggested model {suggested})")
    if config["ram_warning"]:
        notes.append("Warning: limited RAM detected — consider Tailscale or cloud help")
    return config, notes


def _suggest_local_model(ram_gb: float) -> str:
    if ram_gb >= 64:
        return "qwen3-coder:30b"
    if ram_gb >= 32:
        return "qwen3:14b"
    if ram_gb >= 16:
        return "qwen3:7b"
    return "qwen3:4b"


def _max_tokens_for_ram(ram_gb: float) -> int:
    if ram_gb >= 64:
        return 8192
    if ram_gb >= 32:
        return 4096
    if ram_gb >= 16:
        return 2048
    return 1024


def generate_env_config(config: dict[str, Any]) -> str:
    """Render a .env snippet from the suggested configuration."""

    lines = ["# Auto-generated by hardware_detector.py", f"# Mode: {config['mode'].upper()}", ""]
    lines.append(f"OLLAMA_HOST={config['ollama_host']}")
    lines.append(f"OLLAMA_MODEL={config['ollama_model']}")

    if config["mode"] == "dragon":
        lines += ["OLLAMA_SERVE_EXTERNAL=1", "MAX_NEW_TOKENS=8192"]
    elif config["mode"] == "team":
        if config.get("local_fallback"):
            lines.append(f"OLLAMA_FALLBACK_MODEL={config['local_fallback']}")
        lines.append(f"MAX_NEW_TOKENS={config['max_new_tokens']}")
    else:
        if config.get("ram_warning"):
            lines.append("# WARNING: consider upgrading RAM or using Dragon via Tailscale")
        lines.append(f"MAX_NEW_TOKENS={config['max_new_tokens']}")

    return "\n".join(lines)


def _run_cli() -> None:
    logging.basicConfig(level=os.environ.get("LOG_LEVEL", "INFO"))
    print("VistaScribe Hardware Detection\n")
    profile = build_hardware_profile()
    config, summary = suggest_configuration(profile)
    print("\n".join(summary))
    print("\nSUGGESTED CONFIGURATION:\n" + "=" * 50)
    env_config = generate_env_config(config)
    print(env_config)
    print("\n" + "=" * 50)
    save = input("Save this configuration to .env.suggested? (y/n): ")
    if save.lower().startswith("y"):
        Path(".env.suggested").write_text(env_config)
        print("Saved to .env.suggested — review and rename to .env when ready.")


if __name__ == "__main__":
    _run_cli()
