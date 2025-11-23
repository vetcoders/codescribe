#!/usr/bin/env python3
"""VistaScribeServer – dedicated entrypoint for the local FastAPI backend.

Responsibilities handled here instead of ad-hoc in shell scripts:

* Single-instance guarding via PID lock file
* Stable process title (shows up in Activity Monitor / top)
* Deterministic port selection with graceful fallback list
* Recording the chosen port for tray clients (`logs/vistascribe-server.port`)

The actual FastAPI application still lives in :mod:`backend`.  We import it
after configuring logging/env so the existing lazy model loaders reuse the
selected settings.
"""

from __future__ import annotations

import argparse
import atexit
import errno
import logging
import os
import signal
import socket
import sys
from collections.abc import Iterable

from .path_utils import repo_root

logger = logging.getLogger(__name__)

try:
    from setproctitle import setproctitle as _set_proc_title_fn
except ImportError:  # pragma: no cover
    _set_proc_title_fn = None  # type: ignore


DEFAULT_PORTS: tuple[int, ...] = (8237, 7237, 6237, 5237)

REPO_ROOT = repo_root()
PID_DIR = REPO_ROOT / ".pids"
PID_FILE = PID_DIR / "vistascribe-server.pid"
LOG_DIR = REPO_ROOT / "logs"
PORT_FILE = LOG_DIR / "vistascribe-server.port"


def _ensure_dirs() -> None:
    PID_DIR.mkdir(parents=True, exist_ok=True)
    LOG_DIR.mkdir(parents=True, exist_ok=True)


def _parse_ports(raw: Iterable[int] | str | None) -> list[int]:
    if raw is None:
        return list(DEFAULT_PORTS)
    if isinstance(raw, str):
        parts = [p.strip() for p in raw.split(",") if p.strip()]
    else:
        parts = [str(p) for p in raw]
    ports: list[int] = []
    for fragment in parts:
        try:
            value = int(fragment)
        except ValueError as exc:  # pragma: no cover - guarded in CLI
            raise argparse.ArgumentTypeError(f"Invalid port value: {fragment}") from exc
        if value not in ports:
            ports.append(value)
    return ports


def _can_bind(host: str, port: int) -> bool:
    """Return True if the address can be bound."""

    try:
        addrinfo = socket.getaddrinfo(
            host,
            port,
            socket.AF_UNSPEC,
            socket.SOCK_STREAM,
            0,
            socket.AI_PASSIVE,
        )
    except socket.gaierror:
        addrinfo = [(socket.AF_INET, socket.SOCK_STREAM, 0, "", (host, port))]

    for family, socktype, proto, _, sockaddr in addrinfo:
        try:
            with socket.socket(family, socktype, proto) as sock:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                sock.bind(sockaddr)
        except OSError:
            return False
        else:
            return True
    return False


def _choose_port(host: str, requested: int | None, fallbacks: list[int]) -> int:
    candidates: list[int] = []
    if requested is not None:
        candidates.append(requested)
    for value in fallbacks:
        if value not in candidates:
            candidates.append(value)

    for port in candidates:
        if _can_bind(host, port):
            return port
    raise RuntimeError(
        f"No free port available for {host}; checked {', '.join(map(str, candidates))}"
    )


def _cleanup_files(*_: object) -> None:
    try:
        PID_FILE.unlink(missing_ok=True)
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)
    try:
        PORT_FILE.unlink(missing_ok=True)
    except Exception as exc:
        logger.debug("Suppressed exception", exc_info=exc)


def _write_pid() -> None:
    PID_FILE.write_text(str(os.getpid()))
    try:
        PID_FILE.chmod(0o600)
    except OSError as exc:
        logger.debug("PID file chmod failed", exc_info=exc)
    atexit.register(_cleanup_files)


def _ensure_single_instance() -> None:
    if not PID_FILE.exists():
        _write_pid()
        return

    try:
        pid_text = PID_FILE.read_text().strip()
        existing_pid = int(pid_text)
    except Exception:
        PID_FILE.unlink(missing_ok=True)
        _write_pid()
        return

    try:
        os.kill(existing_pid, 0)
    except OSError as exc:
        if exc.errno in (errno.ESRCH, errno.ENOENT):
            # stale lock
            PID_FILE.unlink(missing_ok=True)
            _write_pid()
            return
        raise
    else:
        raise SystemExit(
            f"VistaScribeServer already running (pid {existing_pid}); use --status/--stop first"
        )


def _configure_logging(level: str) -> None:
    root = logging.getLogger()
    if root.handlers:
        return
    logging.basicConfig(
        level=level.upper(),
        format="%(asctime)s - %(levelname)s - %(message)s",
    )


def _handle_signals() -> None:
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, lambda *_: sys.exit(0))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run the VistaScribe backend server")
    parser.add_argument("command", nargs="?", default="start", choices={"start", "status", "stop"})
    parser.add_argument("--bind", default=os.environ.get("VISTASCRIBE_BIND", "127.0.0.1"))
    parser.add_argument("--port", type=int, default=None)
    parser.add_argument(
        "--port-fallbacks",
        default=os.environ.get("VISTASCRIBE_PORT_FALLBACKS", ",".join(map(str, DEFAULT_PORTS))),
        help="Comma separated list of fallback ports (highest priority first)",
    )
    parser.add_argument("--log-level", default=os.environ.get("LOG_LEVEL", "INFO"))

    args = parser.parse_args(argv)

    if args.command == "status":
        if PID_FILE.exists():
            try:
                pid_text = PID_FILE.read_text().strip()
                pid_value = int(pid_text)
                os.kill(pid_value, 0)
                status = "running"
            except Exception:
                status = "stale"
        else:
            status = "stopped"
        port_info = PORT_FILE.read_text().strip() if PORT_FILE.exists() else "?"
        print(f"VistaScribeServer: {status} (port {port_info})")
        return 0

    if args.command == "stop":
        if PID_FILE.exists():
            try:
                pid = int(PID_FILE.read_text().strip())
                os.kill(pid, signal.SIGTERM)
                print(f"Sent SIGTERM to VistaScribeServer (pid {pid})")
            except Exception as exc:
                print(f"Could not stop server cleanly: {exc}", file=sys.stderr)
        else:
            print("VistaScribeServer is not running")
        return 0

    # command == "start"
    fallback_ports = _parse_ports(args.port_fallbacks)

    # Configure logging before backend import so it reuses the chosen level
    os.environ.setdefault("LOG_LEVEL", args.log_level.upper())
    _configure_logging(args.log_level)

    _ensure_dirs()
    _ensure_single_instance()
    _handle_signals()

    bind_host = args.bind
    chosen_port = _choose_port(bind_host, args.port, fallback_ports)
    PORT_FILE.write_text(str(chosen_port))
    try:
        PORT_FILE.chmod(0o600)
    except OSError as exc:
        logger.debug("Port file chmod failed", exc_info=exc)

    if _set_proc_title_fn is not None:
        try:
            _set_proc_title_fn("VistaScribeServer")
        except Exception as exc:
            logger.debug("Suppressed exception", exc_info=exc)

    logging.getLogger(__name__).info("Starting VistaScribeServer on %s:%s", bind_host, chosen_port)

    import uvicorn

    # Delay backend import until now so logging + env are settled
    app_import = "backend:app"

    config = uvicorn.Config(
        app_import,
        host=bind_host,
        port=chosen_port,
        reload=False,
        log_level=args.log_level.lower(),
        access_log=False,
    )
    server = uvicorn.Server(config)

    try:
        server.run()
        return 0
    finally:
        _cleanup_files()


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
