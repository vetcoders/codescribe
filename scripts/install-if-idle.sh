#!/usr/bin/env bash
# Install the local app only when no Codescribe take is in flight.
# Bus authority: session_started without a later session_ended (lifecycle
# terminal written by the controller on every path back to Idle) → refuse.
# The legacy transcript_sealed marker is still honoured for older buses.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Keep the guard on the same path authority as the Rust runtime and diagnostic
# consumers. A guard that inspects a different file is fail-open by definition.
BUS="$(python3 "$ROOT/scripts/bus-demux.py" --print-bus-path)"

installation_unsafe() {
  [[ ! -e "$BUS" ]] || [[ -f "$BUS" ]] || return 0
  [[ -f "$BUS" ]] || return 1
  local bus_status=0
  python3 - "$BUS" <<'PY' || bus_status=$?
import json, sys
path = sys.argv[1]
session = None
sealed = True
try:
    with open(path, encoding="utf-8", errors="strict") as handle:
        for raw in handle:
            raw = raw.strip()
            if not raw:
                continue
            try:
                event = json.loads(raw)
            except json.JSONDecodeError:
                raise SystemExit(0)
            if not isinstance(event, dict):
                raise SystemExit(0)
            status = event.get("status")
            if status == "session_started":
                candidate = event.get("session_id")
                if not isinstance(candidate, str) or not candidate:
                    raise SystemExit(0)
                session = candidate
                sealed = False
                continue
            if session is None or event.get("session_id") != session:
                continue
            if status in ("session_ended", "transcript_sealed"):
                sealed = True
except (OSError, UnicodeDecodeError):
    raise SystemExit(0)
# 0 means unsafe. 3 is the only affirmative idle proof; every unexpected
# interpreter failure is therefore also unsafe to install through.
raise SystemExit(0 if (session is not None and not sealed) else 3)
PY
  [[ "$bus_status" -eq 3 ]] && return 1
  return 0
}

if installation_unsafe; then
  echo "install-if-idle: refuse — Transcript Bus is live or unreadable" >&2
  exit 2
fi

echo "install-if-idle: idle — make install-app"
exec make -C "$ROOT" install-app
