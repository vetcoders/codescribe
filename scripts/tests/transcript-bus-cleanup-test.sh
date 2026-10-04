#!/usr/bin/env bash
# W3: bash scripts/tests/transcript-bus-cleanup-test.sh
# Exercise the real fixture cleanup, never an installed app or existing PID.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
exec python3 - "$ROOT" <<'PY'
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
sandbox = Path(tempfile.mkdtemp(prefix="codescribe cleanup "))
(sandbox / "home").mkdir()
(sandbox / "tmp").mkdir()
# Do not inherit a real dotenv, Bus, lock path or fake-make protocol.
env = {
    "HOME": str(sandbox / "home"),
    "TMPDIR": str(sandbox / "tmp"),
    "PATH": str(Path(shutil.which("python3")).parent) + os.pathsep + os.defpath,
}

# Require an explicit parent even on hosts where bare mktemp honors TMPDIR.
# The shim validates arguments, then uses the real utility to allocate; the
# readiness assertion below independently checks the actual resolved parent.
real_mktemp = shutil.which("mktemp", path=env["PATH"])
assert real_mktemp is not None
allocation_bin = sandbox / "allocation bin"
allocation_bin.mkdir()
allocator = allocation_bin / "mktemp"
allocator.write_text("""#!/usr/bin/env python3
import os
from pathlib import Path
import sys

if (len(sys.argv) != 3 or sys.argv[1] != "-d"
        or not Path(sys.argv[2]).is_absolute()
        or Path(sys.argv[2]).parent.resolve() != Path(os.environ["TMPDIR"]).resolve()):
    print("explicit-parent regression: missing or foreign template parent", file=sys.stderr)
    raise SystemExit(65)
os.execv(""" + repr(real_mktemp) + """, ["mktemp", *sys.argv[1:]])
""")
allocator.chmod(0o700)
env["PATH"] = str(allocation_bin) + os.pathsep + env["PATH"]


def ready_line(process, timeout=15):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise AssertionError("explicit readiness deadline expired")
        line = process.stdout.readline()
    if not line:
        raise AssertionError("process exited before readiness")
    return line.rstrip("\n")


def absent(pid):
    try:
        os.kill(pid, 0)  # observation only; never signal a recorded helper PID
    except ProcessLookupError:
        return True
    return False


# This is deliberately outside every fixture's ownership, with a pipe-based
# exit and finite lifetime even if the regression driver disappears.
sentinel = subprocess.Popen(
    [sys.executable, "-c",
     "import select,sys; print('sentinel-ready', flush=True); "
     "select.select([sys.stdin], [], [], 120)"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env,
)
scenarios = (("normal", 0), ("early-failure", 17), ("TERM", 143), ("INT", 130))
completed = 0
safe_to_remove = True
try:
    assert ready_line(sentinel) == "sentinel-ready"
    # Negative controls allocate nothing, including in the unowned parent.
    for arguments in (("-d",), ("-d", "/tmp/foreign.XXXXXXXX")):
        rejected = subprocess.run([str(allocator), *arguments], env=env,
                                  capture_output=True, text=True, timeout=5)
        assert rejected.returncode == 65 and not rejected.stdout, rejected
        assert "explicit-parent regression" in rejected.stderr, rejected.stderr
    print("cleanup-regression: explicit-parent negative-controls=2", flush=True)
    for name, expected in scenarios:
        fixture = None
        owned = []
        with (sandbox / (name + ".stderr")).open("w+") as errors:
            process = subprocess.Popen(
                ["/bin/bash", str(root / "scripts/tests/transcript-bus-path-test.sh"),
                 "--cleanup-fixture"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                text=True, env=env,
            )
            try:
                fields = ready_line(process).split("\t")
                assert len(fields) == 5 and fields[0] == "fixture-ready", fields
                fixture = Path(fields[1])
                assert fixture.resolve().parent == (sandbox / "tmp").resolve(), fields
                guard, make, lock = owned = [int(value) for value in fields[2:]]
                assert len(set(owned)) == 3 and all(pid > 1 for pid in owned)
                assert str(lock) == (fixture / "lock.ready").read_text()
                assert str(make) == (fixture / "make.ready").read_text()
                assert all(not absent(pid) for pid in owned), "helper died before trigger"
                assert not (fixture / "make.stopped").exists()
                assert not (fixture / "make.release").exists()
                assert sentinel.poll() is None, "negative control died before trigger"
                started = time.monotonic()
                if name in ("TERM", "INT"):
                    # Popen owns and reaps this exact child; never use a name,
                    # process group, historical PID or helper PID as a target.
                    process.send_signal(getattr(signal, "SIG" + name))
                    output, _ = process.communicate(timeout=25)
                else:
                    command = "complete\n" if name == "normal" else "fail\n"
                    output, _ = process.communicate(command, timeout=25)
                elapsed = time.monotonic() - started
                errors.seek(0)
                stderr = errors.read()
                assert process.returncode == expected, (name, process.returncode, output, stderr)
                assert f"cleanup: original={expected} cleanup=0 root={fixture}" in output, output
                for pid in (guard, lock):
                    assert f"cleanup: reaped {pid} status=0" in output, output
                assert f"cleanup: make-stopped {make}" in output, output
                assert not fixture.exists(), "fixture removed only after terminal/reap receipts"
                assert all(absent(pid) for pid in owned), ("owned orphan", owned)
                assert sentinel.poll() is None, "cleanup killed the negative control"
                if name == "early-failure":
                    assert "deliberate assertion failure" in stderr, stderr
                completed += 1
                print(f"cleanup-regression: {name} status={expected} seconds={elapsed:.3f} "
                      f"guard={guard} make={make} lock={lock} no-owned-orphans "
                      f"sentinel={sentinel.pid}:alive", flush=True)
                print(output, end="", flush=True)
            finally:
                # Failure of the regression itself must not strand its fixture.
                # Closing stdin asks the builtin read to fail and enter EXIT;
                # use fixture-local cancellation only after path validation.
                if fixture is not None and fixture.resolve().parent == (sandbox / "tmp").resolve():
                    if fixture.exists():
                        (fixture / "cancel").touch()
                if process.stdin and not process.stdin.closed:
                    process.stdin.close()
                try:
                    process.wait(timeout=35)
                except subprocess.TimeoutExpired:
                    safe_to_remove = False
                    # Keep the sentinel paths; do not turn a timeout into a
                    # successful regression by force-killing its shell owner.
                    print(f"cleanup-regression: owner {process.pid} did not settle; "
                          f"retained {sandbox}", file=sys.stderr)
                if any(not absent(pid) for pid in owned):
                    safe_to_remove = False
                if process.returncode is None or (fixture is None and process.returncode != 0):
                    safe_to_remove = False
                process.stdout.close()
    assert completed == len(scenarios) and completed > 0
    print(f"cleanup-regression: scenarios={completed} no-owned-orphans=4 negative-controls=4")
finally:
    sentinel.stdin.close()
    try:
        sentinel.wait(timeout=5)
    except subprocess.TimeoutExpired:
        safe_to_remove = False
    sentinel.stdout.close()
    if safe_to_remove and completed == len(scenarios):
        shutil.rmtree(sandbox)
    else:
        print(f"cleanup-regression: retained evidence {sandbox}", file=sys.stderr)
PY
