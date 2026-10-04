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
#
# --from-app PATH copies an already signed and stapled Release bundle after
# a running old app has exited. Without that flag, make install-app stays.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "$ROOT" "$@" <<'PY'
import errno
import fcntl
import os
from pathlib import Path
import plistlib
import subprocess
import sys
import time

root = Path(sys.argv[1])
demux = root / "scripts" / "bus-demux.py"
DEST = Path("/Applications/Codescribe.app")
BUNDLE_ID = "com.vetcoders.codescribe"
EXPECTED_TEAM = "MW223P3NPX"
EXPECTED_AUTHORITY = f"Developer ID Application: Maciej Gad ({EXPECTED_TEAM})"
BRIDGE_MARKERS = ("macos/Codescribe/Bridge/", "Codescribe/Bridge/")


class Refuse(Exception):
    pass


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


def assert_bus_idle() -> None:
    bus_check = subprocess.run([sys.executable, str(demux), "--assert-install-idle"])
    if bus_check.returncode != 0:
        print(
            "install-if-idle: refuse — a take is recording (Transcript Bus live or unreadable)",
            file=sys.stderr,
        )
        raise SystemExit(2)


def assert_turn_idle(lease_path: Path) -> None:
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


def open_exclusive_probe(path: Path) -> tuple[int, bool]:
    """Probe shared holders by attempting a non-blocking exclusive lock.

    The default lane keeps going when a running app holds the lock shared.
    The prebuilt lane admits a copy only when this returns held=True.
    """
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        if error.errno in (errno.EACCES, errno.EAGAIN):
            return descriptor, False
        os.close(descriptor)
        raise
    return descriptor, True


def request_quit() -> None:
    quit_result = subprocess.run(
        ["osascript", "-e", 'quit app "Codescribe"'], capture_output=True, text=True
    )
    if quit_result.returncode != 0:
        print(f"install-if-idle: Quit request failed: {quit_result.stderr.strip()}")


def parse_from_app(argv: list[str]) -> Path | None:
    if not argv:
        return None
    if len(argv) == 2 and argv[0] == "--from-app" and argv[1]:
        return Path(argv[1]).expanduser()
    print(
        "install-if-idle: refuse — usage: install-if-idle.sh [--from-app PATH]",
        file=sys.stderr,
    )
    raise SystemExit(2)


def cargo_version() -> str:
    try:
        lines = (root / "Cargo.toml").read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise Refuse(
            f"current Cargo package version is unreadable ({error.strerror or error.errno})"
        ) from error
    for line in lines:
        if line.startswith('version = "') and line.endswith('"'):
            return line[len('version = "') : -1]
    raise Refuse("current Cargo package version is missing")


def current_clean_commit() -> str:
    inside = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--is-inside-work-tree"],
        capture_output=True,
        text=True,
    )
    if inside.returncode != 0 or inside.stdout.strip() != "true":
        raise Refuse("current checkout is not a git worktree")
    status = subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "status",
            "--porcelain",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
        capture_output=True,
        text=True,
    )
    if status.returncode != 0:
        raise Refuse("current checkout status is unreadable")
    for line in status.stdout.splitlines():
        if any(marker in line for marker in BRIDGE_MARKERS):
            continue
        if line.strip():
            raise Refuse("current checkout is dirty; CSBuildCommit must be the clean short SHA")
    shown = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--short=9", "HEAD"],
        capture_output=True,
        text=True,
    )
    if shown.returncode != 0:
        raise Refuse("current commit is unreadable")
    commit = shown.stdout.strip()
    if len(commit) < 9 or any(character not in "0123456789abcdef" for character in commit):
        raise Refuse("current commit is not a clean short SHA")
    return commit


def is_install_destination(path: Path) -> bool:
    dest = DEST.resolve()
    try:
        resolved = path.resolve()
    except OSError as error:
        raise Refuse(f"app path is not readable ({error.strerror or error.errno})") from error
    if resolved == dest or path == DEST:
        return True
    try:
        resolved.relative_to(dest)
        return True
    except ValueError:
        pass
    try:
        dest.relative_to(resolved)
        resolved.relative_to(Path("/Applications"))
    except ValueError:
        return False
    return True


def read_bundle_plist(bundle: Path) -> dict:
    if bundle.is_symlink() or not bundle.is_dir() or not bundle.name.endswith(".app"):
        raise Refuse("not an ordinary app folder")
    if not os.access(bundle, os.R_OK | os.X_OK):
        raise Refuse("app folder is not readable")
    plist_path = bundle / "Contents" / "Info.plist"
    if plist_path.is_symlink() or not plist_path.is_file() or not os.access(plist_path, os.R_OK):
        raise Refuse("app plist is not an ordinary readable file")
    try:
        with plist_path.open("rb") as handle:
            loaded = plistlib.load(handle)
    except (OSError, plistlib.InvalidFileException, ValueError):
        raise Refuse("app plist is not readable")
    if not isinstance(loaded, dict):
        raise Refuse("app plist is not readable")
    return loaded


def require_tool(argv: list[str], reason: str) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(argv, capture_output=True, text=True)
    if completed.returncode != 0:
        raise Refuse(reason)
    return completed


def require_signing(bundle: Path) -> None:
    require_tool(
        ["codesign", "--verify", "--deep", "--strict", str(bundle)],
        "strict deep signature verification failed",
    )
    shown = require_tool(
        ["codesign", "--display", "--verbose=4", str(bundle)],
        "signing metadata is unreadable",
    )
    teams: list[str] = []
    authorities: list[str] = []
    for line in f"{shown.stdout}\n{shown.stderr}".splitlines():
        if line.startswith("TeamIdentifier="):
            teams.append(line.split("=", 1)[1].strip())
        elif line.startswith("Authority="):
            authorities.append(line.split("=", 1)[1].strip())
    if not teams or any(team != EXPECTED_TEAM for team in teams):
        raise Refuse(f"signing team is not Maciej Gad ({EXPECTED_TEAM})")
    if EXPECTED_AUTHORITY not in authorities:
        raise Refuse(f"signing authority is not Developer ID Application for Maciej Gad ({EXPECTED_TEAM})")
    require_tool(
        ["xcrun", "stapler", "validate", str(bundle)],
        "stapler validation failed",
    )
    assessed = subprocess.run(
        ["spctl", "--assess", "--type", "execute", "--verbose=4", str(bundle)],
        capture_output=True,
        text=True,
    )
    assessment = f"{assessed.stdout}\n{assessed.stderr}".lower()
    if assessed.returncode != 0 or "accepted" not in assessment:
        raise Refuse("Gatekeeper assessment failed")


def require_identity(info: dict) -> None:
    if info.get("CFBundleIdentifier") != BUNDLE_ID:
        raise Refuse(f"bundle id is not {BUNDLE_ID}")
    if info.get("CFBundleShortVersionString") != cargo_version():
        raise Refuse("version does not match the current Cargo package version")
    if info.get("CSBuildCommit") != current_clean_commit():
        raise Refuse("CSBuildCommit does not match the current clean git short SHA")


def validate_bundle(bundle: Path, *, allow_destination: bool) -> None:
    if not allow_destination and is_install_destination(bundle):
        raise Refuse("source is the /Applications destination")
    require_identity(read_bundle_plist(bundle))
    require_signing(bundle)


def copy_prebuilt(source: Path, destination: Path) -> None:
    help_run = subprocess.run(["rsync", "--help"], capture_output=True, text=True)
    help_text = f"{help_run.stdout}\n{help_run.stderr}"
    command = ["rsync", "-a"]
    if "--xattrs" in help_text:
        command.extend(["--xattrs", "--delete", "--"])
    else:
        command.extend(["-E", "--delete"])
    command.extend([f"{source}/", f"{destination}/"])
    completed = subprocess.run(command, capture_output=True, text=True)
    if completed.returncode != 0:
        raise Refuse("rsync copy failed")


def install_local(lease_path: Path) -> None:
    interlock_path = resolved_path("--print-install-interlock-path")
    interlock, held = open_exclusive_probe(interlock_path)
    if held:
        print("install-if-idle: no live take, no agent turn, runtime idle — make install-app")
    else:
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
            request_quit()
            remaining = wait_for_old_generation(old_pids)
            if remaining:
                print("install-if-idle: restart required, old generation still running pid="
                      + ",".join(sorted(remaining)))
                raise SystemExit(0)
        print("install-if-idle: old generation exited; installed app is ready to launch")
    else:
        print("install-if-idle: installed app is ready to launch")
    raise SystemExit(0)


def release_descriptor(descriptor: int) -> None:
    try:
        fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)


def install_prebuilt(source: Path, lease_path: Path) -> None:
    # Validation above is not authority for a later bus or turn state.
    assert_bus_idle()
    assert_turn_idle(lease_path)
    old_pids = running_app_pids()
    if old_pids:
        assert_bus_idle()
        assert_turn_idle(lease_path)
        if old_pids & running_app_pids():
            request_quit()
            remaining = wait_for_old_generation(old_pids)
            if remaining:
                raise Refuse(
                    "old generation still running pid=" + ",".join(sorted(remaining))
                )
        print("install-if-idle: old generation exited")
    assert_bus_idle()
    assert_turn_idle(lease_path)
    interlock_path = resolved_path("--print-install-interlock-path")
    interlock, held = open_exclusive_probe(interlock_path)
    if not held:
        os.close(interlock)
        raise Refuse(
            "runtime install interlock unavailable (lock held by "
            f"{lock_holders(interlock_path)})"
        )
    try:
        flags = fcntl.fcntl(interlock, fcntl.F_GETFD)
        fcntl.fcntl(interlock, fcntl.F_SETFD, flags | fcntl.FD_CLOEXEC)
        assert_bus_idle()
        assert_turn_idle(lease_path)
        print(
            "install-if-idle: no live take, no agent turn, runtime interlock held — copying prebuilt bundle"
        )
        destination = DEST
        destination.parent.mkdir(parents=True, exist_ok=True)
        copy_prebuilt(source, destination)
        validate_bundle(destination, allow_destination=True)
        validate_bundle(source, allow_destination=False)
    finally:
        release_descriptor(interlock)
    print("install-if-idle: prebuilt bundle copied; launch not requested")
    raise SystemExit(0)


try:
    from_app = parse_from_app(sys.argv[2:])
    if from_app is not None:
        validate_bundle(from_app, allow_destination=False)
    assert_bus_idle()
    lease_path = resolved_path("--print-agent-turn-lease-path")
    assert_turn_idle(lease_path)
    if from_app is None:
        install_local(lease_path)
    else:
        install_prebuilt(from_app, lease_path)
except Refuse as error:
    print(f"install-if-idle: refuse — {error}", file=sys.stderr)
    raise SystemExit(2)
except (OSError, subprocess.SubprocessError) as error:
    print(f"install-if-idle: refuse — interlock check failed: {error}", file=sys.stderr)
    raise SystemExit(2)
PY
