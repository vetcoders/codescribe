#!/usr/bin/env bash
# The local install never quits the application itself. `make install-app`
# arms restart-if-idle, whose native protocol owns the swap; the installer
# watches that swap and reports what became of the running generation.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
mkdir -p "$WORKDIR/bin" "$WORKDIR/data" "$WORKDIR/home"
: > "$WORKDIR/data/transcript-bus.jsonl"

# `make install-app` stands in for the build and for the waiter it arms. The
# waiter is detached, so its swap lands after make has returned.
cat > "$WORKDIR/bin/make" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$TEST_MAKE_LOG"
swap() {
  (
    sleep 0.6
    printf '%s' "$1" > "$TEST_PIDS_FILE.next"
    mv "$TEST_PIDS_FILE.next" "$TEST_PIDS_FILE"
  ) > /dev/null 2>&1 &
}
case "${TEST_WAITER:-idle}" in
  restart) swap $'5252\n' ;;
  vanish) swap '' ;;
esac
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
SH
cat > "$WORKDIR/bin/open" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$TEST_OPEN_LOG"
if [[ "${TEST_OPEN_MODE:-start}" == "start" ]]; then printf '%s\n' 6262 > "$TEST_PIDS_FILE"; fi
SH
chmod +x "$WORKDIR/bin/make" "$WORKDIR/bin/pgrep" "$WORKDIR/bin/osascript" "$WORKDIR/bin/open"

# The runtime lease and install interlock resolve under HOME: an isolated one
# keeps the probes off the locks of the application actually running here.
export HOME="$WORKDIR/home"
export PATH="$WORKDIR/bin:$PATH"
export CODESCRIBE_DATA_DIR="$WORKDIR/data"
export CODESCRIBE_ENV_PATH="$WORKDIR/data/.env"
export CODESCRIBE_TRANSCRIPT_BUS_PATH="$WORKDIR/data/transcript-bus.jsonl"
export CODESCRIBE_INSTALL_SWAP_WAIT=2
export TEST_PIDS_FILE="$WORKDIR/pids"
export TEST_MAKE_LOG="$WORKDIR/make.log"
export TEST_QUIT_LOG="$WORKDIR/quit.log"
export TEST_OPEN_LOG="$WORKDIR/open.log"
: > "$TEST_PIDS_FILE"
: > "$TEST_MAKE_LOG"
: > "$TEST_QUIT_LOG"
: > "$TEST_OPEN_LOG"

# Nothing was running: nothing is started on the Founder's behalf.
out="$("$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"installed app is ready to launch"* ]]
[[ ! -s "$TEST_OPEN_LOG" ]]

# The waiter swapped the generation: the installer names the new process.
printf '%s\n' 4242 > "$TEST_PIDS_FILE"
out="$(TEST_WAITER=restart "$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"old generation exited; new generation running pid=5252"* ]]
[[ ! -s "$TEST_OPEN_LOG" ]]

# The application is busy: it keeps running its build until the waiter's
# idle moment. The installer waits out its window and says so.
printf '%s\n' 4343 > "$TEST_PIDS_FILE"
out="$("$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"restart required; old generation still running pid=4343"* ]]
[[ "$(cat "$TEST_PIDS_FILE")" == 4343 ]]
[[ ! -s "$TEST_OPEN_LOG" ]]

# The old generation left and no successor came: an application that was
# running when the install finished copying is running when it returns.
printf '%s\n' 4444 > "$TEST_PIDS_FILE"
out="$(TEST_WAITER=vanish "$ROOT/scripts/install-if-idle.sh")"
[[ "$out" == *"old generation exited without a successor; started the installed app pid=6262"* ]]
[[ "$(cat "$TEST_OPEN_LOG")" == "-g /Applications/Codescribe.app" ]]

# ... and when even that launch fails, the install does not pass as done.
printf '%s\n' 4545 > "$TEST_PIDS_FILE"
: > "$TEST_OPEN_LOG"
if TEST_WAITER=vanish TEST_OPEN_MODE=fail "$ROOT/scripts/install-if-idle.sh" \
  > "$WORKDIR/off.out" 2> "$WORKDIR/off.err"; then
  echo "an application left off must fail the install" >&2
  exit 1
fi
[[ "$(cat "$WORKDIR/off.err")" == *"the installed app is not running"* ]]
[[ -s "$TEST_OPEN_LOG" ]]

# A failed build propagates and starts nothing.
printf '%s\n' 4646 > "$TEST_PIDS_FILE"
: > "$TEST_OPEN_LOG"
if TEST_MAKE_CODE=7 "$ROOT/scripts/install-if-idle.sh" > "$WORKDIR/failed.log"; then
  echo "install should propagate make failure" >&2
  exit 1
fi
[[ ! -s "$TEST_OPEN_LOG" ]]

# In every case above: no Quit of the installer's own.
[[ ! -s "$TEST_QUIT_LOG" ]]

echo "install-if-idle-quit: ok"
