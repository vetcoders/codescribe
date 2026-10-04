#!/usr/bin/env bash
# Install the local app only when Codescribe is not busy.
#
# Busy means exactly two things (Founder, 2026-09-08):
#   1. a take is recording — Bus authority: the most recently started app
#      session without a later session_ended (or legacy transcript_sealed),
#      plus any unpaired cli_file_verdict session (the CLI holds no lock);
#   2. an agent turn is in flight — the app holds ~/.codescribe/agent-turn.lock
#      shared for the whole turn (streaming, tools, Stop, persist).
# A merely running app does NOT block installation. Historical unpaired
# starts are abandoned, not live.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$ROOT" <<'PY'
import errno
import fcntl
import os
from pathlib import Path
import subprocess
import sys
import time

root = Path(sys.argv[1])
demux = root / "scripts" / "bus-demux.py"

def running_app_pids() -> set[str]:
    result = subprocess.run(["pgrep", "-x", "Codescribe"], capture_output=True, text=True)
    if result.returncode not in (0, 1):
        raise subprocess.SubprocessError(f"pgrep failed: {result.stderr.strip()}")
    return {pid for pid in result.stdout.split() if pid.isdecimal()}

def wait_for_old_generation(old_pids: set[str]) -> set[str]:
    deadline = time.monotonic() + 30
    while True:
        remaining = old_pids & running_app_pids()
        if not remaining or time.monotonic() >= deadline:
            return remaining
        time.sleep(0.25)

def lock_holders(path: Path) -> str:
    try:
        found = subprocess.run(["lsof", "-t", "--", str(path)], capture_output=True, text=True)
        pids = sorted(set(found.stdout.split()))
        if not pids:
            return "PID and command unavailable"
        holders = []
        for pid in pids:
            command = subprocess.run(["ps", "-p", pid, "-o", "command="], capture_output=True, text=True)
            holders.append(f"PID {pid} ({command.stdout.strip() or 'command unavailable'})")
        return ", ".join(holders)
    except OSError:
        return "PID and command unavailable"

def resolved_path(flag: str) -> Path:
    result = subprocess.run(
        [sys.executable, str(demux), flag],
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(result.stdout.strip())

try:
    bus_check = subprocess.run([sys.executable, str(demux), "--assert-install-idle"])
    if bus_check.returncode != 0:
        print(
            "install-if-idle: refuse — a take is recording (Transcript Bus live or unreadable)",
            file=sys.stderr,
        )
        raise SystemExit(2)

    # The app holds this shared only while an agent turn runs. Probing it
    # exclusively (non-blocking) tells us whether a turn is in flight; the
    # probe is released immediately so a turn starting later is never refused.
    lease_path = resolved_path("--print-agent-turn-lease-path")
    if lease_path.parent != Path("."):
        lease_path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(lease_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        if error.errno in (errno.EACCES, errno.EAGAIN):
            print(
                f"install-if-idle: refuse — an agent turn is in flight (lock held by {lock_holders(lease_path)})",
                file=sys.stderr,
            )
            raise SystemExit(2)
        raise
    finally:
        os.close(descriptor)

    # Best-effort exclusive lease on the runtime interlock for the copy window:
    # when nothing runs, a *new* app start is refused until the bundle is
    # whole. A running app holds it shared — that no longer refuses install,
    # so we proceed without the lease and say so.
    interlock_path = resolved_path("--print-install-interlock-path")
    interlock = os.open(interlock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(interlock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        print("install-if-idle: no live take, no agent turn, runtime idle — make install-app")
    except OSError as error:
        if error.errno not in (errno.EACCES, errno.EAGAIN):
            raise
        print(
            f"install-if-idle: no live take, no agent turn; app is running (runtime lock held by {lock_holders(interlock_path)}) — "
            "installing over it, then requesting Quit"
        )
    try:
        completed = subprocess.run(["make", "-C", str(root), "install-app"])
    finally:
        os.close(interlock)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)

    old_pids = running_app_pids()
    if old_pids:
        # A take or agent turn may have begun during the build. Never request
        # Quit unless the bus and turn lease still prove the app is idle.
        if subprocess.run([sys.executable, str(demux), "--assert-install-idle"]).returncode != 0:
            print("install-if-idle: restart required; old generation still running pid="
                  + ",".join(sorted(old_pids)) + " (take active or bus unreadable)")
            raise SystemExit(0)
        lease = os.open(lease_path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            try:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as error:
                if error.errno in (errno.EACCES, errno.EAGAIN):
                    print("install-if-idle: restart required; old generation still running pid="
                          + ",".join(sorted(old_pids)) + " (agent turn active)")
                    raise SystemExit(0)
                raise
        finally:
            os.close(lease)

        if old_pids & running_app_pids():
            quit_result = subprocess.run(
                ["osascript", "-e", 'quit app "Codescribe"'], capture_output=True, text=True
            )
            if quit_result.returncode != 0:
                print(f"install-if-idle: Quit request failed: {quit_result.stderr.strip()}")
            remaining = wait_for_old_generation(old_pids)
            if remaining:
                print("install-if-idle: restart required, old generation still running pid="
                      + ",".join(sorted(remaining)))
                raise SystemExit(0)
        print("install-if-idle: old generation exited; installed app is ready to launch")
    else:
        print("install-if-idle: installed app is ready to launch")
    raise SystemExit(0)
except (OSError, subprocess.SubprocessError) as error:
    print(f"install-if-idle: refuse — interlock check failed: {error}", file=sys.stderr)
    raise SystemExit(2)
PY
