#!/usr/bin/env bash
# Hermetic parity checks for the runtime Bus path and install-if-idle guard.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEMUX="$ROOT/scripts/bus-demux.py"
INSTALL_GUARD="$ROOT/scripts/install-if-idle.sh"
# Supply the parent in the template: bare mktemp may choose a platform default.
TEST_TMP_PARENT="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "${TEST_TMP_PARENT%/}/transcript-bus-path.XXXXXXXX")"
LOCK_HOLDER_PID=""
GUARD_PID=""
# Cancellation is fixture-local data, never a PID/name/process-group kill.
# Keep the root (and release paths) until Bash has reaped every owned job.
cleanup() {
  local original_status=$? cleanup_status=0 pid child_status
  trap '' TERM INT
  trap - EXIT
  set +e
  touch "$TEST_ROOT/cancel" || cleanup_status=1
  for _ in {1..1000}; do
    jobs -pr >"$TEST_ROOT/running-jobs"
    jobs -ps >>"$TEST_ROOT/running-jobs"
    [[ -s "$TEST_ROOT/running-jobs" ]] || break
    sleep 0.01
  done
  if [[ -s "$TEST_ROOT/running-jobs" ]]; then
    echo "transcript-bus-path: cleanup timed out; retained $TEST_ROOT" >&2
    cleanup_status=1
  else
    for pid in "$LOCK_HOLDER_PID" "$GUARD_PID"; do
      [[ -n "$pid" ]] || continue
      wait "$pid"
      child_status=$?
      printf 'cleanup: reaped %s status=%s\n' "$pid" "$child_status"
      [[ "$child_status" -eq 0 ]] || cleanup_status=1
    done
    wait # also reap a job interrupted between spawn and PID assignment
    if [[ -e "$TEST_ROOT/make.ready" ]]; then
      if [[ -e "$TEST_ROOT/make.stopped" ]]; then
        printf 'cleanup: make-stopped %s\n' "$(<"$TEST_ROOT/make.stopped")"
      else
        echo "transcript-bus-path: missing fake make terminal receipt" >&2
        cleanup_status=1
      fi
    fi
    if [[ "$cleanup_status" -eq 0 ]]; then
      rm -rf "$TEST_ROOT" || cleanup_status=1
    else
      echo "transcript-bus-path: cleanup failed; retained $TEST_ROOT" >&2
    fi
  fi
  printf 'cleanup: original=%s cleanup=%s root=%s\n' \
    "$original_status" "$cleanup_status" "$TEST_ROOT"
  [[ "$original_status" -eq 0 ]] || exit "$original_status"
  exit "$cleanup_status"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

unset FAKE_MAKE_READY FAKE_MAKE_RELEASE
TEST_HOME="$TEST_ROOT/home"
mkdir -p "$TEST_HOME"

resolve_bus() {
  env \
    -u CODESCRIBE_TRANSCRIPT_BUS_PATH \
    -u CODESCRIBE_TRANSCRIPT_BUS \
    -u CODESCRIBE_ENV_PATH \
    -u XDG_STATE_HOME \
    -u CODESCRIBE_DATA_DIR \
    HOME="$TEST_HOME" \
    "$@" \
    python3 "$DEMUX" --print-bus-path
}

resolve_interlock() {
  env \
    -u CODESCRIBE_TRANSCRIPT_BUS_PATH \
    -u CODESCRIBE_TRANSCRIPT_BUS \
    -u CODESCRIBE_ENV_PATH \
    -u XDG_STATE_HOME \
    -u CODESCRIBE_DATA_DIR \
    HOME="$TEST_HOME" \
    "$@" \
    python3 "$DEMUX" --print-install-interlock-path
}

assert_path() {
  local expected="$1"
  shift
  local observed
  observed="$(resolve_bus "$@")"
  if [[ "$observed" != "$expected" ]]; then
    echo "transcript-bus-path: expected $expected, observed $observed" >&2
    exit 1
  fi
}

assert_path "$TEST_HOME/.codescribe/transcript-events.jsonl"
assert_path \
  "$TEST_HOME/data-root/transcript-events.jsonl" \
  CODESCRIBE_DATA_DIR="~/data-root"
assert_path \
  "$TEST_HOME/state-root/codescribe/transcript-events.jsonl" \
  CODESCRIBE_DATA_DIR="$TEST_ROOT/data-root" \
  XDG_STATE_HOME="~/state-root"
assert_path \
  "$TEST_HOME/direct/transcript.jsonl" \
  CODESCRIBE_DATA_DIR="$TEST_ROOT/data-root" \
  XDG_STATE_HOME="$TEST_ROOT/state-root" \
  CODESCRIBE_TRANSCRIPT_BUS_PATH="~/direct/transcript.jsonl"

# The legacy alias is deliberately ignored because the Rust runtime never
# reads it. Accepting it in a guard would inspect a different authority file.
assert_path \
  "$TEST_HOME/.codescribe/transcript-events.jsonl" \
  CODESCRIBE_TRANSCRIPT_BUS="$TEST_ROOT/legacy.jsonl"

# Runtime bootstrap reads the optional dotenv before resolving the Bus. These
# keys are env-managed (not promoted settings), so the guard must see them too.
mkdir -p "$TEST_HOME/.codescribe"
printf '%s\n' \
  'CODESCRIBE_TRANSCRIPT_BUS_PATH=~/dotenv/transcript.jsonl' \
  >"$TEST_HOME/.codescribe/.env"
assert_path "$TEST_HOME/dotenv/transcript.jsonl"
assert_path \
  "$TEST_HOME/process-wins.jsonl" \
  CODESCRIBE_TRANSCRIPT_BUS_PATH="~/process-wins.jsonl"
rm -f "$TEST_HOME/.codescribe/.env"

CUSTOM_ENV="$TEST_ROOT/custom.env"
printf '%s\n' 'XDG_STATE_HOME=~/dotenv-state' >"$CUSTOM_ENV"
assert_path \
  "$TEST_HOME/dotenv-state/codescribe/transcript-events.jsonl" \
  CODESCRIBE_ENV_PATH="$CUSTOM_ENV"

# Config::config_dir treats presence as authority even when the value is empty
# or whitespace, and canonicalizes existing non-empty paths.
assert_path "transcript-events.jsonl" CODESCRIBE_DATA_DIR=""
assert_path "  /transcript-events.jsonl" CODESCRIBE_DATA_DIR="  "
mkdir -p "$TEST_ROOT/canonical-data"
ln -s "$TEST_ROOT/canonical-data" "$TEST_ROOT/data-link"
CANONICAL_DATA="$(cd "$TEST_ROOT/canonical-data" && pwd -P)"
assert_path \
  "$CANONICAL_DATA/transcript-events.jsonl" \
  CODESCRIBE_DATA_DIR="$TEST_ROOT/data-link"
if [[ "$(resolve_interlock)" != "$TEST_HOME/.codescribe/install-runtime.lock" ]]; then
  echo "transcript-bus-path: default interlock path diverged" >&2
  exit 1
fi
if [[ "$(resolve_interlock CODESCRIBE_DATA_DIR="$TEST_ROOT/data-link")" != "$TEST_HOME/.codescribe/install-runtime.lock" ]]; then
  echo "transcript-bus-path: data-dir override relocated invariant interlock" >&2
  exit 1
fi
if [[ "$(resolve_interlock CODESCRIBE_ENV_PATH="$CUSTOM_ENV")" != "$TEST_HOME/.codescribe/install-runtime.lock" ]]; then
  echo "transcript-bus-path: dotenv bootstrap relocated invariant interlock" >&2
  exit 1
fi

OPEN_BUS="$TEST_ROOT/open/transcript-events.jsonl"
mkdir -p "$(dirname "$OPEN_BUS")"
python3 - "$OPEN_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    handle.write(
        json.dumps(
            {
                "schema": "codescribe.transcript.v1",
                "session_id": "live-session",
                "status": "session_started",
            }
        )
        + "\n"
    )
PY

FAKE_BIN="$TEST_ROOT/bin"
FAKE_MAKE_LOG="$TEST_ROOT/fake-make.log"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/make" <<'PYMAKE'
#!/usr/bin/env python3
# No sleep subprocess: this helper owns no descendants.
import os
from pathlib import Path
import sys
import time

root = Path(__file__).resolve().parent.parent
ready = os.environ.get("FAKE_MAKE_READY")
if ready:
    deadline = time.monotonic() + 30
    Path(ready + ".pending").write_text(str(os.getpid()))
    Path(ready + ".pending").replace(ready)
    while not Path(os.environ["FAKE_MAKE_RELEASE"]).exists():
        if (root / "cancel").exists():
            break
        if not root.exists() or time.monotonic() >= deadline:
            raise SystemExit("fake make: owner disappeared or release deadline expired")
        time.sleep(0.01)
Path(os.environ["FAKE_MAKE_LOG"]).write_text(" ".join(sys.argv[1:]) + "\n")
if ready:
    (root / "make.stopped").write_text(str(os.getpid()))
PYMAKE
chmod +x "$FAKE_BIN/make"

hold_shared_lock() {
  local path="$1" ready="$2" release="$3"
  python3 - "$path" "$ready" "$release" "$TEST_ROOT" <<'PY' &
import fcntl
import os
from pathlib import Path
import sys
import time

path = Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
with path.open("a+") as handle:
    fcntl.flock(handle.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
    root = Path(sys.argv[4])
    deadline = time.monotonic() + 30
    ready = Path(sys.argv[2])
    ready.with_suffix(".pending").write_text(str(os.getpid()))
    ready.with_suffix(".pending").replace(ready)
    while not Path(sys.argv[3]).exists():
        if (root / "cancel").exists():
            break
        if not root.exists() or time.monotonic() >= deadline:
            raise SystemExit("lock holder: owner disappeared or release deadline expired")
        time.sleep(0.01)
PY
  LOCK_HOLDER_PID=$!
  for _ in {1..500}; do
    [[ ! -e "$ready" ]] || break
    sleep 0.01
  done
  if [[ ! -e "$ready" ]]; then
    echo "transcript-bus-path: lock holder for $path did not start" >&2
    exit 1
  fi
}

start_idle_guard() {
  env \
    -u CODESCRIBE_TRANSCRIPT_BUS_PATH \
    -u CODESCRIBE_ENV_PATH \
    -u XDG_STATE_HOME \
    -u CODESCRIBE_DATA_DIR \
    HOME="$TEST_HOME" \
    PATH="$FAKE_BIN:$PATH" \
    FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
    FAKE_MAKE_READY="$MAKE_READY" \
    FAKE_MAKE_RELEASE="$MAKE_RELEASE" \
    CODESCRIBE_TRANSCRIPT_BUS="$TEST_ROOT/legacy.jsonl" \
    "$INSTALL_GUARD" >"$TEST_ROOT/idle.out" 2>"$TEST_ROOT/idle.err" &
  GUARD_PID=$!
  for _ in {1..500}; do
    [[ ! -e "$MAKE_READY" ]] || break
    sleep 0.01
  done
  if [[ ! -e "$MAKE_READY" ]]; then
    echo "transcript-bus-path: fake make did not start under install lease" >&2
    exit 1
  fi
}

# Standalone cleanup regression uses these same helpers and EXIT trap. It
# stops before policy assertions so a policy failure cannot hide this census.
if [[ "${1:-}" == "--cleanup-fixture" ]]; then
  MAKE_READY="$TEST_ROOT/make.ready"
  MAKE_RELEASE="$TEST_ROOT/make.release"
  hold_shared_lock "$TEST_ROOT/fixture.lock" "$TEST_ROOT/lock.ready" "$TEST_ROOT/lock.release"
  start_idle_guard
  printf 'fixture-ready\t%s\t%s\t%s\t%s\n' \
    "$TEST_ROOT" "$GUARD_PID" "$(<"$MAKE_READY")" "$LOCK_HOLDER_PID"
  # A builtin wait permits Bash to run INT/TERM traps immediately. Python's
  # parent launches us with default signal dispositions, unlike shell `&` INT.
  if ! IFS= read -r -t 20 fixture_command; then
    echo "transcript-bus-path: fixture controller disappeared/timed out" >&2
    exit 124
  fi
  case "$fixture_command" in
    complete) exit 0 ;;
    fail) echo "transcript-bus-path: deliberate assertion failure" >&2; exit 17 ;;
    *) echo "transcript-bus-path: invalid fixture command" >&2; exit 64 ;;
  esac
fi

assert_guard_refuses() {
  local bus="$1"
  local label="$2"
  rm -f "$FAKE_MAKE_LOG"
  set +e
  env \
    -u CODESCRIBE_ENV_PATH \
    -u XDG_STATE_HOME \
    -u CODESCRIBE_DATA_DIR \
    HOME="$TEST_HOME" \
    PATH="$FAKE_BIN:$PATH" \
    FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
    CODESCRIBE_TRANSCRIPT_BUS_PATH="$bus" \
    "$INSTALL_GUARD" >"$TEST_ROOT/$label.out" 2>"$TEST_ROOT/$label.err"
  local guard_status=$?
  set -e
  if [[ "$guard_status" -ne 2 ]]; then
    echo "transcript-bus-path: $label guard returned $guard_status, expected 2" >&2
    exit 1
  fi
  if [[ -e "$FAKE_MAKE_LOG" ]]; then
    echo "transcript-bus-path: install command ran for $label Bus" >&2
    exit 1
  fi
}

assert_guard_refuses "$OPEN_BUS" "live"

# A Bus path injected only by the app dotenv is still canonical authority.
printf '%s\n' "CODESCRIBE_TRANSCRIPT_BUS_PATH=$OPEN_BUS" \
  >"$TEST_HOME/.codescribe/.env"
rm -f "$FAKE_MAKE_LOG"
set +e
env \
  -u CODESCRIBE_TRANSCRIPT_BUS_PATH \
  -u CODESCRIBE_ENV_PATH \
  -u XDG_STATE_HOME \
  -u CODESCRIBE_DATA_DIR \
  HOME="$TEST_HOME" \
  PATH="$FAKE_BIN:$PATH" \
  FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
  "$INSTALL_GUARD" >"$TEST_ROOT/dotenv-live.out" 2>"$TEST_ROOT/dotenv-live.err"
dotenv_guard_status=$?
set -e
rm -f "$TEST_HOME/.codescribe/.env"
if [[ "$dotenv_guard_status" -ne 2 ]] || [[ -e "$FAKE_MAKE_LOG" ]]; then
  echo "transcript-bus-path: dotenv-only live Bus did not fail closed" >&2
  exit 1
fi

# An open start cannot age out of an arbitrary tail window.
DEEP_OPEN_BUS="$TEST_ROOT/deep-open.jsonl"
python3 - "$DEEP_OPEN_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    handle.write(json.dumps({"session_id": "deep", "status": "session_started"}) + "\n")
    for sequence in range(4_100):
        handle.write(json.dumps({"sequence": sequence, "status": "utterance_draft"}) + "\n")
PY
assert_guard_refuses "$DEEP_OPEN_BUS" "deep-live"

# Closing a nested CLI file-verdict must not hide an older still-open app take.
# The CLI does not hold the install flock; the app session is the live mic.
NESTED_OPEN_BUS="$TEST_ROOT/nested-open.jsonl"
python3 - "$NESTED_OPEN_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    for session_id, status, source in (
        ("outer", "session_started", None),
        ("inner", "session_started", "cli_file_verdict"),
        ("inner", "session_ended", "cli_file_verdict"),
    ):
        event = {"session_id": session_id, "status": status}
        if source is not None:
            event["source"] = source
        handle.write(json.dumps(event) + "\n")
PY
assert_guard_refuses "$NESTED_OPEN_BUS" "nested-live"

# Historical unpaired starts plus a later completed app take are idle.
# One microphone: those old rows are abandoned, not a live recording.
ABANDONED_BUS="$TEST_ROOT/abandoned.jsonl"
python3 - "$ABANDONED_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    for session_id in ("abandoned-a", "abandoned-b", "abandoned-c"):
        handle.write(json.dumps({"session_id": session_id, "status": "session_started"}) + "\n")
    handle.write(json.dumps({"session_id": "current", "status": "session_started"}) + "\n")
    handle.write(json.dumps({"session_id": "current", "status": "session_ended"}) + "\n")
PY
set +e
env \
  -u CODESCRIBE_ENV_PATH \
  -u XDG_STATE_HOME \
  -u CODESCRIBE_DATA_DIR \
  HOME="$TEST_HOME" \
  python3 "$DEMUX" --bus "$ABANDONED_BUS" --assert-install-idle
abandoned_idle_status=$?
set -e
if [[ "$abandoned_idle_status" -ne 0 ]]; then
  echo "transcript-bus-path: abandoned historical starts still looked live" >&2
  exit 1
fi

# A malformed authority file is not evidence of idle.
MALFORMED_BUS="$TEST_ROOT/malformed.jsonl"
printf '%s\n' '{"status":"session_started"' >"$MALFORMED_BUS"
assert_guard_refuses "$MALFORMED_BUS" "malformed"

INVALID_UTF8_BUS="$TEST_ROOT/invalid-utf8.jsonl"
python3 - "$INVALID_UTF8_BUS" <<'PY'
from pathlib import Path
import sys

Path(sys.argv[1]).write_bytes(b"\xff\n")
PY
assert_guard_refuses "$INVALID_UTF8_BUS" "invalid-utf8"

# A lifecycle terminal for the latest session is positive idle evidence.
CLOSED_BUS="$TEST_ROOT/closed.jsonl"
python3 - "$CLOSED_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    for status in ("session_started", "session_ended"):
        handle.write(json.dumps({"session_id": "closed", "status": status}) + "\n")
PY

resolve_agent_turn_lease() {
  env \
    -u CODESCRIBE_ENV_PATH \
    -u XDG_STATE_HOME \
    -u CODESCRIBE_DATA_DIR \
    HOME="$TEST_HOME" \
    python3 "$DEMUX" --print-agent-turn-lease-path
}
if [[ "$(resolve_agent_turn_lease)" != "$TEST_HOME/.codescribe/agent-turn.lock" ]]; then
  echo "transcript-bus-path: agent-turn lease path diverged" >&2
  exit 1
fi

# A merely running app (shared process-lifetime lease on the runtime
# interlock) must NOT refuse installation (Founder, 2026-09-08).
INTERLOCK_PATH="$(resolve_interlock)"
hold_shared_lock "$INTERLOCK_PATH" "$TEST_ROOT/app-lock.ready" "$TEST_ROOT/app-lock.release"
rm -f "$FAKE_MAKE_LOG"
env \
  -u CODESCRIBE_ENV_PATH \
  -u XDG_STATE_HOME \
  -u CODESCRIBE_DATA_DIR \
  HOME="$TEST_HOME" \
  PATH="$FAKE_BIN:$PATH" \
  FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
  CODESCRIBE_TRANSCRIPT_BUS_PATH="$CLOSED_BUS" \
  "$INSTALL_GUARD" >"$TEST_ROOT/runtime-running.out" 2>"$TEST_ROOT/runtime-running.err"
if [[ ! -e "$FAKE_MAKE_LOG" ]]; then
  echo "transcript-bus-path: a running app (no take, no agent turn) blocked install" >&2
  exit 1
fi
touch "$TEST_ROOT/app-lock.release"
wait "$LOCK_HOLDER_PID"
LOCK_HOLDER_PID=""

# An agent turn in flight holds the agent-turn lease shared. The installer
# must refuse even though the Bus is closed and the runtime lock is free.
AGENT_TURN_LEASE_PATH="$(resolve_agent_turn_lease)"
hold_shared_lock "$AGENT_TURN_LEASE_PATH" "$TEST_ROOT/turn-lock.ready" "$TEST_ROOT/turn-lock.release"
assert_guard_refuses "$CLOSED_BUS" "agent-turn"
if ! grep -q 'agent turn is in flight' "$TEST_ROOT/agent-turn.err"; then
  echo "transcript-bus-path: agent-turn refusal did not name the agent turn" >&2
  exit 1
fi
touch "$TEST_ROOT/turn-lock.release"
wait "$LOCK_HOLDER_PID"
LOCK_HOLDER_PID=""

rm -f "$FAKE_MAKE_LOG"
env \
  -u CODESCRIBE_ENV_PATH \
  -u XDG_STATE_HOME \
  -u CODESCRIBE_DATA_DIR \
  HOME="$TEST_HOME" \
  PATH="$FAKE_BIN:$PATH" \
  FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
  CODESCRIBE_TRANSCRIPT_BUS_PATH="$CLOSED_BUS" \
  "$INSTALL_GUARD" >"$TEST_ROOT/closed.out" 2>"$TEST_ROOT/closed.err"
if [[ ! -e "$FAKE_MAKE_LOG" ]]; then
  echo "transcript-bus-path: closed Bus did not reach isolated make" >&2
  exit 1
fi

# With no canonical Bus, the guard may proceed to the isolated fake make. A
# live file reachable only through the retired alias must not redirect it.
cp "$OPEN_BUS" "$TEST_ROOT/legacy.jsonl"
rm -f "$FAKE_MAKE_LOG"
MAKE_READY="$TEST_ROOT/make.ready"
MAKE_RELEASE="$TEST_ROOT/make.release"
start_idle_guard
set +e
python3 - "$INTERLOCK_PATH" <<'PY'
import fcntl
from pathlib import Path
import sys

with Path(sys.argv[1]).open("a+") as handle:
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
    except BlockingIOError:
        raise SystemExit(3)
raise SystemExit(0)
PY
shared_during_install_status=$?
set -e
if [[ "$shared_during_install_status" -ne 3 ]]; then
  echo "transcript-bus-path: app lease was admitted during install" >&2
  exit 1
fi
touch "$MAKE_RELEASE"
wait "$GUARD_PID"
GUARD_PID=""

expected_make="-C $ROOT install-app"
observed_make="$(<"$FAKE_MAKE_LOG")"
if [[ "$observed_make" != "$expected_make" ]]; then
  echo "transcript-bus-path: fake make observed '$observed_make'" >&2
  exit 1
fi

echo "transcript-bus-path: ok"
