#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
mkdir -p "$WORKDIR/bin" "$WORKDIR/data"
: > "$WORKDIR/data/transcript-bus.jsonl"

cat > "$WORKDIR/bin/make" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$TEST_MAKE_LOG"
exit "${TEST_MAKE_CODE:-0}"
SH
cat > "$WORKDIR/bin/pgrep" <<'SH'
#!/usr/bin/env bash
[[ "$*" == "-x Codescribe" ]] || exit 2
[[ -s "$TEST_PIDS_FILE" ]] || exit 1
cat "$TEST_PIDS_FILE"
SH
cat > "$WORKDIR/bin/osascript" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$TEST_QUIT_LOG"
if [[ "$TEST_QUIT_MODE" == "exit" ]]; then : > "$TEST_PIDS_FILE"; fi
SH
chmod +x "$WORKDIR/bin/make" "$WORKDIR/bin/pgrep" "$WORKDIR/bin/osascript"

export PATH="$WORKDIR/bin:$PATH"
export CODESCRIBE_DATA_DIR="$WORKDIR/data"
export CODESCRIBE_ENV_PATH="$WORKDIR/data/.env"
export CODESCRIBE_TRANSCRIPT_BUS_PATH="$WORKDIR/data/transcript-bus.jsonl"
export TEST_PIDS_FILE="$WORKDIR/pids"
export TEST_MAKE_LOG="$WORKDIR/make.log"
export TEST_QUIT_LOG="$WORKDIR/quit.log"
export TEST_QUIT_MODE=exit
: > "$TEST_PIDS_FILE"
: > "$TEST_MAKE_LOG"
: > "$TEST_QUIT_LOG"

out="$("$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"installed app is ready to launch"* ]]
[[ ! -s "$TEST_QUIT_LOG" ]]

printf '%s\n' 4242 > "$TEST_PIDS_FILE"
out="$("$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"old generation exited"* ]]
[[ "$(cat "$TEST_QUIT_LOG")" == '-e quit app "Codescribe"' ]]
[[ ! -s "$TEST_PIDS_FILE" ]]

printf '%s\n' 4343 > "$TEST_PIDS_FILE"
TEST_QUIT_MODE=linger out="$(TEST_QUIT_MODE=linger "$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"restart required, old generation still running pid=4343"* ]]
[[ -s "$TEST_PIDS_FILE" ]]

printf '%s\n' 4444 > "$TEST_PIDS_FILE"
before="$(wc -l < "$TEST_QUIT_LOG")"
if TEST_MAKE_CODE=7 "$ROOT/scripts/install-if-idle.sh" > "$WORKDIR/failed.log"; then
  echo "install should propagate make failure" >&2
  exit 1
fi
[[ "$(wc -l < "$TEST_QUIT_LOG")" == "$before" ]]

echo "install-if-idle-quit: ok"
