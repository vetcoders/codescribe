#!/usr/bin/env bash
# Install the local app only when no Codescribe take is in flight.
# Bus authority: session_started without a later session_ended (lifecycle
# terminal written by the controller on every path back to Idle) → refuse.
# The legacy transcript_sealed marker is still honoured for older buses.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$ROOT" <<'PY'
import errno
import fcntl
import os
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
demux = root / "scripts" / "bus-demux.py"

def resolved_path(flag: str) -> Path:
    result = subprocess.run(
        [sys.executable, str(demux), flag],
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(result.stdout.strip())

try:
    interlock_path = resolved_path("--print-install-interlock-path")
    if interlock_path.parent != Path("."):
        interlock_path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(interlock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        if error.errno in (errno.EACCES, errno.EAGAIN):
            print(
                "install-if-idle: refuse — Codescribe application runtime is active",
                file=sys.stderr,
            )
            raise SystemExit(2)
        raise

    bus_check = subprocess.run([sys.executable, str(demux), "--assert-install-idle"])
    if bus_check.returncode != 0:
        print(
            "install-if-idle: refuse — Transcript Bus is live or unreadable",
            file=sys.stderr,
        )
        raise SystemExit(2)

    print("install-if-idle: exclusive lease + idle Bus — make install-app")
    completed = subprocess.run(["make", "-C", str(root), "install-app"])
    raise SystemExit(completed.returncode)
except (OSError, subprocess.SubprocessError) as error:
    print(f"install-if-idle: refuse — interlock check failed: {error}", file=sys.stderr)
    raise SystemExit(2)
PY
