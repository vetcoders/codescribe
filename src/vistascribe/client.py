"""Lightweight HTTP client for VistaScribe server operations.

This module provides HTTP-based transcription and formatting to keep the tray process light.
All heavy ML operations are delegated to the VistaScribeServer process.
"""

from __future__ import annotations

import asyncio
import logging
import os
import random
import subprocess
import sys
import time
import uuid
from contextlib import suppress
from importlib import metadata
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import httpx

from .formatting import apply_light_plus
from .path_utils import repo_root
from .settings_store import get_settings

try:
    __version__ = metadata.version("vistascribe")
except Exception:  # pragma: no cover - local editable installs may miss metadata
    __version__ = "dev"

DEFAULT_PORTS: tuple[int, ...] = (8237, 7237, 6237, 5237)
USER_AGENT = f"VistaScribeClient/{__version__}"
ALLOW_REMOTE_ENV = os.environ.get("ALLOW_REMOTE_VISTASCRIBE") or os.environ.get("ALLOW_REMOTE")
PORT_FILE = repo_root() / "logs" / "vistascribe-server.port"


def _backend_host() -> str:
    host = os.environ.get("VISTASCRIBE_HOST", "127.0.0.1").strip()
    return host or "127.0.0.1"


def _format_host(host: str) -> str:
    if ":" in host and not host.startswith("[") and not host.endswith("]"):
        return f"[{host}]"
    return host


_async_clients: dict[int, httpx.AsyncClient] = {}
_async_loops: dict[int, asyncio.AbstractEventLoop] = {}
_sync_client: httpx.Client | None = None
_cached_url_async: str | None = None
_cached_url_sync: str | None = None

TRANSIENT_STATUS_CODES = {502, 503, 504}


def _is_truthy(value: str | None) -> bool:
    if value is None:
        return False
    return value.strip().lower() not in {"", "0", "false", "no", "off"}


def _normalize_url(value: str | None) -> str | None:
    if not value:
        return None
    stripped = value.strip().rstrip("/")
    return stripped or None


def _is_loopback_host(host: str | None) -> bool:
    if host is None:
        return False
    host = host.lower()
    return host in {"localhost", "::1"} or host.startswith("127.")


def _remote_allowed(url: str) -> bool:
    parsed = urlparse(url)
    if _is_loopback_host(parsed.hostname):
        return True
    return _is_truthy(ALLOW_REMOTE_ENV)


def _explicit_url_candidate() -> str | None:
    candidate = _normalize_url(os.environ.get("VISTASCRIBE_SERVER_URL"))
    if not candidate:
        return None
    if not _remote_allowed(candidate):
        logging.error(
            "Remote VistaScribeServer URLs are disabled. "
            "Set ALLOW_REMOTE_VISTASCRIBE=1 to override."
        )
        return None
    return candidate


def _port_file_candidate() -> str | None:
    if not PORT_FILE.exists():
        return None
    with suppress(Exception):
        port = PORT_FILE.read_text(encoding="utf-8").strip()
        if port.isdigit():
            host = _backend_host()
            candidate = f"http://{_format_host(host)}:{port}"
            if _remote_allowed(candidate):
                return candidate
    return None


def _candidate_urls() -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []

    for candidate in (_explicit_url_candidate(), _port_file_candidate()):
        if candidate and candidate not in seen:
            ordered.append(candidate)
            seen.add(candidate)

    host = _backend_host()
    for port in DEFAULT_PORTS:
        candidate = f"http://{_format_host(host)}:{port}"
        if candidate not in seen and _remote_allowed(candidate):
            ordered.append(candidate)
            seen.add(candidate)

    return ordered


def _get_async_client() -> httpx.AsyncClient:
    loop = asyncio.get_event_loop()
    loop_id = id(loop)
    if loop_id in _async_clients and not loop.is_closed():
        return _async_clients[loop_id]

    # Prune clients whose loops are closed
    for stale_id, stale_loop in list(_async_loops.items()):
        if stale_loop.is_closed():
            client = _async_clients.pop(stale_id, None)
            with suppress(Exception):
                if client is not None:
                    client.close()
            _async_loops.pop(stale_id, None)

    client = httpx.AsyncClient(headers={"User-Agent": USER_AGENT})
    _async_clients[loop_id] = client
    _async_loops[loop_id] = loop
    return client


def _get_sync_client() -> httpx.Client:
    global _sync_client
    if _sync_client is None:
        _sync_client = httpx.Client(headers={"User-Agent": USER_AGENT})
    return _sync_client


async def _probe_async(url: str, timeout: float = 0.4) -> bool:
    client = _get_async_client()
    try:
        resp = await client.get(f"{url}/healthz", timeout=httpx.Timeout(timeout, connect=timeout))
        resp.raise_for_status()
        data = resp.json()
        return isinstance(data, dict) and data.get("ok") is True
    except Exception:
        return False


def _probe_sync(url: str, timeout: float = 0.4) -> bool:
    client = _get_sync_client()
    try:
        resp = client.get(f"{url}/healthz", timeout=httpx.Timeout(timeout, connect=timeout))
        resp.raise_for_status()
        data = resp.json()
        return isinstance(data, dict) and data.get("ok") is True
    except Exception:
        return False


async def resolve_server_url(force_refresh: bool = False) -> str | None:
    """Resolve a reachable backend URL asynchronously (prefers cached value)."""

    global _cached_url_async
    if not force_refresh and _cached_url_async:
        if await _probe_async(_cached_url_async, timeout=0.2):
            return _cached_url_async
        _cached_url_async = None

    for candidate in _candidate_urls():
        if await _probe_async(candidate):
            _cached_url_async = candidate
            return candidate
    return None


def resolve_server_url_sync(force_refresh: bool = False) -> str | None:
    """Resolve backend URL synchronously (used by health probes / startup)."""

    global _cached_url_sync
    if not force_refresh and _cached_url_sync and _probe_sync(_cached_url_sync, timeout=0.2):
        return _cached_url_sync
    _cached_url_sync = None

    for candidate in _candidate_urls():
        if _probe_sync(candidate):
            _cached_url_sync = candidate
            return candidate
    return None


def _timeout_from_env(prefix: str, *, total: float, connect: float, read: float) -> httpx.Timeout:
    def _float(env_key: str, default: float) -> float:
        raw = os.environ.get(env_key)
        if raw is None:
            return default
        with suppress(ValueError):
            return float(raw)
        return default

    total_v = _float(f"{prefix}_TIMEOUT", total)
    connect_v = _float(f"{prefix}_CONNECT_TIMEOUT", connect)
    read_v = _float(f"{prefix}_READ_TIMEOUT", read)
    write_v = _float(f"{prefix}_WRITE_TIMEOUT", read_v)
    return httpx.Timeout(timeout=total_v, connect=connect_v, read=read_v, write=write_v)


def _request_headers(pipeline: str) -> dict[str, str]:
    return {
        "X-Request-ID": str(uuid.uuid4()),
        "X-Text-Pipeline": pipeline,
        "User-Agent": USER_AGENT,
    }


async def _request_with_retries(
    method: str,
    url: str,
    *,
    client: httpx.AsyncClient,
    max_attempts: int,
    timeout: httpx.Timeout,
    headers: dict[str, str],
    **kwargs: Any,
) -> httpx.Response:
    attempt = 0
    while True:
        try:
            response = await client.request(
                method,
                url,
                timeout=timeout,
                headers=headers,
                **kwargs,
            )
            if response.status_code in TRANSIENT_STATUS_CODES and attempt + 1 < max_attempts:
                delay = min(1.0, 0.2 * (2**attempt) + random.uniform(0.05, 0.15))
                logging.warning(
                    "Transient HTTP %s for %s; retrying in %.2fs (attempt %s/%s)",
                    response.status_code,
                    url,
                    delay,
                    attempt + 1,
                    max_attempts,
                )
                await asyncio.sleep(delay)
                attempt += 1
                continue
            response.raise_for_status()
            return response
        except httpx.HTTPStatusError:
            raise
        except httpx.RequestError as exc:
            if attempt + 1 >= max_attempts:
                raise
            delay = min(1.0, 0.2 * (2**attempt) + random.uniform(0.05, 0.2))
            logging.warning(
                "Request error for %s (%s); retrying in %.2fs (attempt %s/%s)",
                url,
                exc,
                delay,
                attempt + 1,
                max_attempts,
            )
            await asyncio.sleep(delay)
            attempt += 1


async def transcribe_http(audio_path: str, language: str | None = None) -> str | None:
    """Transcribe audio via HTTP without buffering the entire file in memory."""

    server_url = await resolve_server_url()
    if not server_url:
        logging.error("VistaScribeServer not found. Please start the server.")
        return None

    timeout = _timeout_from_env("TRANSCRIBE", total=180.0, connect=0.5, read=180.0)
    headers = _request_headers("stt")
    files: dict[str, tuple[str, Any, str]]
    data: dict[str, str] = {}
    if language:
        data["language"] = language

    client = _get_async_client()
    try:
        with open(audio_path, "rb") as audio_file:
            files = {
                "audio": (Path(audio_path).name, audio_file, "audio/wav"),
            }
            response = await _request_with_retries(
                "POST",
                f"{server_url}/transcribe",
                client=client,
                max_attempts=3,
                timeout=timeout,
                headers=headers,
                files=files,
                data=data or None,
            )
        payload = response.json()
        text = payload.get("text") if isinstance(payload, dict) else None
        if text is None:
            logging.error("Transcription response missing text (req=%s)", headers["X-Request-ID"])
            return None
        file_size = Path(audio_path).stat().st_size if os.path.exists(audio_path) else 0
        logging.info(
            "Transcription ok (%s bytes → %s chars, req=%s)",
            file_size,
            len(text),
            headers["X-Request-ID"],
        )
        return text
    except httpx.HTTPStatusError as exc:
        logging.error("Transcription failed (%s) req=%s", exc, headers["X-Request-ID"])
    except httpx.RequestError as exc:
        logging.error("Transcription error (%s) req=%s", exc, headers["X-Request-ID"])
    except Exception as exc:  # pragma: no cover - unexpected edge
        logging.error("Transcription error: %s", exc)
    return None


async def format_text_http(text: str, assistive: bool = False) -> str | None:
    """Format text via HTTP server.

    Pipeline:
    1. ALWAYS apply Light Plus baseline (deterministic cleanup)
    2. If format_strategy is light/light_plus and NOT assistive → return baseline
    3. Otherwise → send baseline to backend for AI enhancement

    Args:
        text: Raw transcription text
        assistive: If True, use assistive AI mode (El Niño) - ALWAYS uses AI

    Returns:
        Formatted text (Light Plus + optional AI enhancement)
    """
    # Step 1: ALWAYS apply Light Plus baseline
    baseline_text = apply_light_plus(text)
    logging.debug(f"Light Plus baseline applied: {len(text)} → {len(baseline_text)} chars")

    # Step 2: Check if AI enhancement is needed
    settings = get_settings()
    use_ai = assistive or settings.ai_formatting_enabled
    if not use_ai:
        logging.info("AI formatting disabled; returning Light+ baseline")
        return baseline_text

    server_url = await resolve_server_url()
    if not server_url:
        logging.warning("VistaScribeServer not found. Returning baseline text.")
        return baseline_text

    defaults = 60.0 if assistive else 30.0
    timeout = _timeout_from_env("FORMAT", total=defaults, connect=0.5, read=defaults)
    headers = _request_headers("light_plus")
    payload = {
        "text": baseline_text,
        "assistive": assistive,
    }

    client = _get_async_client()
    try:
        response = await _request_with_retries(
            "POST",
            f"{server_url}/format",
            client=client,
            max_attempts=2,
            timeout=timeout,
            headers=headers,
            json=payload,
        )
        data = response.json()
        formatted = data.get("text") if isinstance(data, dict) else None
        if formatted:
            logging.info(
                "AI formatting ok (assistive=%s, len=%s, req=%s)",
                assistive,
                len(formatted),
                headers["X-Request-ID"],
            )
            return formatted
        logging.warning(
            "AI formatting missing text; using baseline (req=%s)", headers["X-Request-ID"]
        )
    except httpx.HTTPStatusError as exc:
        logging.error("AI formatting failed (%s); returning baseline", exc)
    except httpx.RequestError as exc:
        logging.error("AI formatting error (%s); returning baseline", exc)
    except Exception as exc:  # pragma: no cover
        logging.error("AI formatting unexpected error (%s); returning baseline", exc)
    return baseline_text


def check_server_status() -> dict[str, bool]:
    """Check if VistaScribeServer is running and what services are available.

    Returns:
        Dictionary with service status: {'server': bool, 'whisper': bool, 'llm': bool}
    """
    status = {"server": False, "whisper": False, "llm": False}

    server_url = resolve_server_url_sync()
    if not server_url:
        return status

    client = _get_sync_client()
    try:
        resp = client.get(
            f"{server_url}/healthz",
            timeout=httpx.Timeout(1.0, connect=0.3),
            headers={"User-Agent": USER_AGENT},
        )
        resp.raise_for_status()
        data = resp.json()
        if isinstance(data, dict) and data.get("ok"):
            status["server"] = True
            status["whisper"] = bool(data.get("whisper", {}).get("ready", True))
            ai_info = data.get("ai", {})
            if isinstance(ai_info, dict):
                status["llm"] = bool(ai_info.get("enabled"))
    except Exception:
        return status

    return status


def _spawn_backend_process() -> bool:
    """Attempt to spawn the dedicated VistaScribeServer entrypoint."""

    python_exe = sys.executable or "python3"
    logs_dir = repo_root() / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    log_path = logs_dir / "vistascribe-server.autostart.log"
    env = os.environ.copy()
    env.setdefault("NOHUP_MODE", "1")

    try:
        log_file = open(log_path, "ab", buffering=0)
    except OSError:
        log_file = subprocess.DEVNULL

    try:
        subprocess.Popen(
            [python_exe, "-m", "vistascribe.vistascribe_server", "start"],
            cwd=str(repo_root()),
            stdout=log_file,
            stderr=log_file,
            env=env,
            start_new_session=True,
        )
    except Exception as exc:
        logging.error("Failed to spawn backend via %s: %s", python_exe, exc)
        return False
    finally:
        if log_file is not subprocess.DEVNULL:
            try:
                log_file.close()
            except Exception:
                pass

    return True


def _spawn_backend_via_launcher() -> bool:
    script_path = repo_root() / "VistaScribe"
    if not script_path.exists():
        return False
    try:
        subprocess.Popen(
            [str(script_path), "start", "backend"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return True
    except Exception as exc:
        logging.error("Failed to invoke %s: %s", script_path, exc)
        return False


def start_server_if_needed() -> bool:
    """Ensure VistaScribeServer is running; auto-start if necessary."""

    if check_server_status()["server"]:
        return True

    spawned = _spawn_backend_process()
    if not spawned:
        spawned = _spawn_backend_via_launcher()
    if not spawned:
        return False

    for _ in range(20):  # Wait up to ~10 seconds
        time.sleep(0.5)
        if check_server_status()["server"]:
            logging.info("VistaScribeServer started successfully")
            return True

    logging.error("Timed out waiting for VistaScribeServer to start")
    return False
