#!/usr/bin/env bash
# One timeline from the Transcript Bus and codescribe.log, ordered by timestamp.
#
#   scripts/bus-tail.sh                       # live: both files, from now on
#   scripts/bus-tail.sh --since 13:17 --until 13:19   # post-hoc window (UTC, today)
#   scripts/bus-tail.sh --since 2026-09-03T13:17:40   # full ISO also accepted
#
#   --warn   keep only WARN/ERROR log lines
#   --all    print every Bus line instead of one per reducer burst
#
# Bus path resolves like the runtime (bus-demux.py --print-bus-path). Log path:
# $CODESCRIBE_LOG_PATH, else $CODESCRIBE_DATA_DIR/logs/codescribe.log.
# Timestamps are UTC as written by both sources; the date is stripped for display.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bus=$(python3 "$here/bus-demux.py" --print-bus-path)
log=${CODESCRIBE_LOG_PATH:-${CODESCRIBE_DATA_DIR:-$HOME/.codescribe}/logs/codescribe.log}

since="" until="" warn=0
while [ $# -gt 0 ]; do
  case $1 in
    --since) since=$2; shift 2 ;;
    --until) until=$2; shift 2 ;;
    --warn) warn=1; shift ;;
    --all) export BUS_ALL=1; shift ;;
    *) sed -n '2,12p' "$0" >&2; exit 2 ;;
  esac
done

today=$(date -u +%Y-%m-%d)
full_ts() { case $1 in *T*) printf '%s' "$1" ;; "") ;; *) printf '%sT%s' "$today" "$1" ;; esac; }
since=$(full_ts "$since")
until=$(full_ts "$until")

bus_lines() { jq -rn --unbuffered -f "$here/bus-brief.jq"; }

# ts level target message — drops thread columns, continuation lines and the
# hotkeys "receipt, not forwarded" echo of every emitter warning.
log_lines() {
  awk -v warn="$warn" '
    $2 !~ /^(TRACE|DEBUG|INFO|WARN|ERROR)$/ { next }
    warn && $2 !~ /^(WARN|ERROR)$/ { next }
    /codescribe_ffi::hotkeys: engine warning \(receipt, not forwarded\)/ { next }
    {
      if ($5 ~ /:$/) { tgt = substr($5, 1, length($5) - 1); n = 5 } else { tgt = "-"; n = 4 }
      msg = ""
      for (i = n + 1; i <= NF; i++) msg = msg (i > n + 1 ? " " : "") $i
      print $1 "\tLOG\t" $2 "\t" tgt "\t" msg
      fflush()
    }'
}

strip_date() { awk '{ sub(/^[0-9-]+T/, ""); print; fflush() }'; }

if [ -n "$since" ] || [ -n "$until" ]; then
  { bus_lines < "$bus"; log_lines < "$log"; } \
    | sort -s -k1,1 \
    | awk -v s="$since" -v u="${until:-9999}" '$1 >= s && $1 <= u' \
    | strip_date
else
  trap 'kill 0' EXIT
  tail -F -n 0 "$bus" | bus_lines | strip_date &
  tail -F -n 0 "$log" | log_lines | strip_date
fi
