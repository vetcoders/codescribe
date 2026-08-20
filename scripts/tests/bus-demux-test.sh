#!/usr/bin/env bash
# Hermetic kielbasa checks for scripts/bus-demux.py. No microphone.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEMUX="$ROOT/scripts/bus-demux.py"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
BUS="$WORKDIR/transcript-events.jsonl"
chmod +x "$DEMUX"

seal() {
  local text="$1"
  python3 - "$BUS" "$text" <<'PY'
import json, sys
path, text = sys.argv[1], sys.argv[2]
event = {
    "schema": "codescribe.transcript.v1",
    "sequence": 1,
    "session_id": "test-session",
    "mode": "dictation",
    "utterance_id": None,
    "emitted_at": "2026-08-20T22:00:00Z",
    "status": "transcript_sealed",
    "text": text,
}
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(event, ensure_ascii=False) + "\n")
PY
}

run_once() {
  python3 "$DEMUX" --bus "$BUS" --once "$@"
}

: >"$BUS"
if python3 "$DEMUX" --bus "$BUS" --once >/dev/null 2>"$WORKDIR/err"; then
  echo "expected unnamed refuse" >&2
  exit 1
fi
grep -q "unnamed agent does not pass" "$WORKDIR/err"

seal "zwykła dyktando do karetki bez imienia"
if run_once --name james >/dev/null 2>/dev/null; then
  echo "expected drop of unnamed seal" >&2
  exit 1
fi

seal "James, wklejka nadal parkuje."
got="$(run_once --name james)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["audience"] == "james", o
assert "parkuje" in o["text"], o
assert o["kind"] == "seal", o
PY

got="$(run_once --all)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["audience"] == "*", o
PY

: >"$BUS"
seal "Cześć James. Będziesz od teraz James."
got="$(run_once --become)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["kind"] == "name_assignment", o
assert o["name"] == "james", o
PY

echo "bus-demux: ok"
