#!/bin/bash
# App screen atlas: Light/Dark screenshots of Codescribe's production windows
# (Overlay, Agent, Settings, Onboarding, Tray) in their states, plus a
# manifest.json and an index.html gallery. See macos/CodescribeTests/README.md.
#
# Usage: scripts/app-screen-atlas.sh [--offscreen-only] [--only SURFACE] [out-dir]
#   --offscreen-only  skip the live compositor captures (no Screen Recording,
#                     works with a locked screen)
#   --only SURFACE    one of Overlay, Agent, Settings, Onboarding, Tray
#   out-dir           default: a timestamped directory under $TMPDIR
#   ATLAS_LANGUAGE / ATLAS_REGION override the interface language (pl / PL).
#
# The test host cannot hold Screen Recording, so this script runs the capture
# loop itself: AppScreenAtlasTests writes "<windowNumber> <name>" to
# <out-dir>/live/request and waits for <out-dir>/live/<name>. Run it from a
# process that holds Screen Recording, with the screen unlocked; otherwise the
# live images are missing and the gallery falls back to the offscreen ones.
set -euo pipefail

usage() {
  sed -n '6,12p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
}

live=1
only=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --offscreen-only) live=0 ;;
    --only)
      [ $# -ge 2 ] || usage
      only="$2"
      shift
      ;;
    -h | --help) usage ;;
    -*) usage ;;
    *)
      [ -z "$out" ] || usage
      out="$1"
      ;;
  esac
  shift
done

case "$only" in
  "" | Overlay | Agent | Settings | Onboarding | Tray) ;;
  *)
    echo "app-screen-atlas: unknown surface '$only'" >&2
    usage
    ;;
esac

if [ -z "$out" ]; then
  out="${TMPDIR:-/tmp}/codescribe-screen-atlas-$(date +%Y%m%d-%H%M%S)"
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
repo="$(cd "$(dirname "$0")/.." && pwd)"

filter="CodescribeTests/AppScreenAtlasTests"
[ -z "$only" ] || filter="$filter/test${only}Atlas"

capturer_pid=""
stop_capturer() {
  if [ -n "$capturer_pid" ]; then
    kill "$capturer_pid" 2>/dev/null || true
    wait "$capturer_pid" 2>/dev/null || true
    capturer_pid=""
  fi
}
trap stop_capturer EXIT INT TERM

# Answers each new request with `screencapture -l`. screencapture refuses
# names that start with a dot, so it writes tmp_<name> and renames it once
# the file is complete.
capture_loop() {
  local dir="$1" last="" req window name
  while true; do
    if [ -f "$dir/request" ]; then
      req="$(cat "$dir/request")"
      if [ -n "$req" ] && [ "$req" != "$last" ]; then
        read -r window name <<<"$req"
        if screencapture -x -o -t png -l "$window" "$dir/tmp_$name" 2>>"$dir/capturer.err"; then
          mv "$dir/tmp_$name" "$dir/$name"
        fi
        last="$req"
      fi
    fi
    sleep 0.2
  done
}

if [ "$live" = 1 ]; then
  mkdir -p "$out/live"
  capture_loop "$out/live" &
  capturer_pid=$!
fi

status=0
(
  cd "$repo"
  TEST_RUNNER_CODESCRIBE_SCREEN_ATLAS_DIR="$out" \
    TEST_RUNNER_CODESCRIBE_SCREEN_ATLAS_LIVE="$live" \
    make test-swift SWIFT_TEST_MAX_TEST_SECONDS=600 \
    SWIFT_TEST_ARGS="-only-testing:$filter -testLanguage ${ATLAS_LANGUAGE:-pl} -testRegion ${ATLAS_REGION:-PL}"
) || status=$?
stop_capturer

offscreen_count=0
live_count=0
if [ -d "$out/offscreen" ]; then
  offscreen_count="$(find "$out/offscreen" -maxdepth 1 -name '*.png' | wc -l | tr -d ' ')"
fi
if [ -d "$out/live" ]; then
  live_count="$(find "$out/live" -maxdepth 1 -name '*.png' ! -name 'tmp_*' | wc -l | tr -d ' ')"
fi
echo "app-screen-atlas: $offscreen_count offscreen, $live_count live captures in $out"
if [ "$live" = 1 ] && [ "$live_count" -lt "$offscreen_count" ]; then
  echo "app-screen-atlas: $((offscreen_count - live_count)) cells have no live capture." >&2
  echo "  Is the screen unlocked, and does this process hold Screen Recording?" >&2
  echo "  The offscreen images are still valid; see $out/live/capturer.err." >&2
fi
if [ -f "$out/index.html" ]; then
  echo "app-screen-atlas: open $out/index.html"
fi
if [ "$status" -ne 0 ]; then
  echo "app-screen-atlas: make test-swift failed with status $status" >&2
fi
exit "$status"
