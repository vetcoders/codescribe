#!/usr/bin/env bash
# Install the local app only when no Codescribe take is in flight.
# Bus authority: session_started without a later transcript_sealed → refuse.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUS="${CODESCRIBE_TRANSCRIPT_BUS:-$HOME/.codescribe/transcript-events.jsonl}"

recording_live() {
  [[ -f "$BUS" ]] || return 1
  python3 - "$BUS" <<'PY'
import json, sys
path = sys.argv[1]
session = None
sealed = True
try:
    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
except OSError:
    raise SystemExit(1)
for raw in lines[-4000:]:
    raw = raw.strip()
    if not raw:
        continue
    try:
        event = json.loads(raw)
    except json.JSONDecodeError:
        continue
    status = event.get("status")
    if status == "session_started":
        session = event.get("session_id")
        sealed = False
        continue
    if session is None:
        continue
    if event.get("session_id") != session:
        continue
    if status == "transcript_sealed":
        sealed = True
raise SystemExit(0 if (session is not None and not sealed) else 1)
PY
}

if recording_live; then
  echo "install-if-idle: refuse — Codescribe take is live (Transcript Bus)" >&2
  exit 2
fi

echo "install-if-idle: idle — make install-app"
exec make -C "$ROOT" install-app
