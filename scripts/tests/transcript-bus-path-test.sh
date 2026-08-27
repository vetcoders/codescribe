#!/usr/bin/env bash
# Hermetic parity checks for the runtime Bus path and install-if-idle guard.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEMUX="$ROOT/scripts/bus-demux.py"
INSTALL_GUARD="$ROOT/scripts/install-if-idle.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

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
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'printf "%s\n" "$*" >"$FAKE_MAKE_LOG"' \
  >"$FAKE_BIN/make"
chmod +x "$FAKE_BIN/make"

assert_guard_refuses() {
  local bus="$1"
  local label="$2"
  rm -f "$FAKE_MAKE_LOG"
  set +e
  env \
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

# Closing the newest nested session must not hide an older still-open take.
NESTED_OPEN_BUS="$TEST_ROOT/nested-open.jsonl"
python3 - "$NESTED_OPEN_BUS" <<'PY'
import json
import sys

with open(sys.argv[1], "w", encoding="utf-8") as handle:
    for session_id, status in (
        ("outer", "session_started"),
        ("inner", "session_started"),
        ("inner", "session_ended"),
    ):
        handle.write(json.dumps({"session_id": session_id, "status": status}) + "\n")
PY
assert_guard_refuses "$NESTED_OPEN_BUS" "nested-live"

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
rm -f "$FAKE_MAKE_LOG"
env \
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
env \
  -u CODESCRIBE_TRANSCRIPT_BUS_PATH \
  -u XDG_STATE_HOME \
  -u CODESCRIBE_DATA_DIR \
  HOME="$TEST_HOME" \
  PATH="$FAKE_BIN:$PATH" \
  FAKE_MAKE_LOG="$FAKE_MAKE_LOG" \
  CODESCRIBE_TRANSCRIPT_BUS="$TEST_ROOT/legacy.jsonl" \
  "$INSTALL_GUARD" >"$TEST_ROOT/idle.out" 2>"$TEST_ROOT/idle.err"

expected_make="-C $ROOT install-app"
observed_make="$(<"$FAKE_MAKE_LOG")"
if [[ "$observed_make" != "$expected_make" ]]; then
  echo "transcript-bus-path: fake make observed '$observed_make'" >&2
  exit 1
fi

echo "transcript-bus-path: ok"
