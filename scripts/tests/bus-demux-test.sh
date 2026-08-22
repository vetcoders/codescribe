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
  local status="${2:-transcript_sealed}"
  local sequence="${3:-1}"
  python3 - "$BUS" "$text" "$status" "$sequence" <<'PY'
import json, sys
path, text, status, sequence = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
event = {
    "schema": "codescribe.transcript.v1",
    "sequence": sequence,
    "session_id": "test-session",
    "mode": "raw",
    "utterance_id": "utterance-1",
    "emitted_at": "2026-08-20T22:00:00Z",
    "status": status,
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
assert o["schema"] == "codescribe.agent-bridge.event.v1", o
assert o["state_change_allowed"] is True, o
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

# A provider-scoped follower emits an attach receipt and all addressed live
# envelopes, then persists the byte cursor. Reattachment consumes only lines
# written after that cursor: no old command replay, no recovery gap.
BRIDGE_HOME="$WORKDIR/agent-bridge"
: >"$BUS"
seal "James, szkic pierwszy." utterance_draft 10
seal "James, szkic poprawiony." utterance_revised 11
seal "James, komenda zamknięta." transcript_sealed 12
first="$WORKDIR/first.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-a --name james \
  --drafts --from-start >"$first"
python3 - "$first" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
assert [row["kind"] for row in rows] == ["attach", "draft", "revised", "seal"], rows
attach = rows[0]
assert attach["resumed"] is False, attach
assert attach["provider"] == "codex", attach
lease_ids = {row["lease_id"] for row in rows}
assert lease_ids == {attach["lease_id"]}, rows
assert rows[1]["state_change_allowed"] is False, rows[1]
assert rows[2]["state_change_allowed"] is False, rows[2]
assert rows[3]["state_change_allowed"] is True, rows[3]
PY

seal "James, komenda po recovery." transcript_sealed 13
second="$WORKDIR/second.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-a --name james \
  --drafts --from-start >"$second"
python3 - "$first" "$second" <<'PY'
import json, sys
first = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
second = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8")]
assert [row["kind"] for row in second] == ["attach", "seal"], second
assert second[0]["resumed"] is True, second[0]
assert second[0]["lease_id"] == first[0]["lease_id"], second[0]
assert second[1]["sequence"] == 13, second[1]
assert "komenda po recovery" in second[1]["text"], second[1]
PY

# Same human name does not collapse provider sessions onto one cursor.
third="$WORKDIR/third.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider claude-code --session claude-session-a --name james \
  --drafts --from-start >"$third"
python3 - "$first" "$third" <<'PY'
import json, sys
first = json.loads(open(sys.argv[1], encoding="utf-8").readline())
third = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8")]
assert third[0]["lease_id"] != first["lease_id"], (first, third[0])
assert third[0]["provider"] == "claude-code", third[0]
assert [row["kind"] for row in third[1:]] == ["draft", "revised", "seal", "seal"], third
PY

# Active-name discovery is lease-derived and cleans stale leases without audio.
python3 - "$DEMUX" "$BUS" "$BRIDGE_HOME" <<'PY'
import importlib.util, json, sys, time
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
lease = module.SessionLease(
    root=Path(sys.argv[3]), provider="codex", provider_session_id="active-session",
    name="iwo", bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
    follow_from_end=True,
)
stale = Path(sys.argv[3]) / "leases" / "stale-lease.json"
module.atomic_json(stale, {
    "schema": module.LEASE_SCHEMA, "lease_id": "stale-lease", "name": "old",
    "active": True, "heartbeat_unix": time.time() - 999,
})
active = module.active_leases(Path(sys.argv[3]), 120, clean=True)
assert {item["name"] for item in active} == {"iwo"}, active
assert not stale.exists(), stale
lease.close()

# --become may bind a name after attach; recovery with that name must reuse the
# provider-session cursor rather than derive a second lease from the new name.
greeting = module.SessionLease(
    root=Path(sys.argv[3]), provider="codex", provider_session_id="become-session",
    name=None, bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
    follow_from_end=True,
)
greeting_id = greeting.lease_id
greeting.bind_name("james")
greeting.close()
recovered = module.SessionLease(
    root=Path(sys.argv[3]), provider="codex", provider_session_id="become-session",
    name="james", bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
    follow_from_end=True,
)
assert recovered.resumed is True, recovered.attach_receipt()
assert recovered.lease_id == greeting_id, recovered.attach_receipt()
assert recovered.name == "james", recovered.attach_receipt()
recovered.close()
PY

# The provider/session identity is protected by a stable advisory lock. A
# second follower cannot win a simultaneous stale-read race or fork the cursor.
collision_out="$WORKDIR/collision-first.jsonl"
collision_err="$WORKDIR/collision-first.err"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session collision-session --name james \
  --drafts --follow >"$collision_out" 2>"$collision_err" &
collision_pid=$!
for _ in {1..100}; do
  if [[ -s "$collision_out" ]]; then
    break
  fi
  sleep 0.01
done
if [[ ! -s "$collision_out" ]]; then
  echo "first collision follower did not attach" >&2
  kill "$collision_pid" 2>/dev/null || true
  wait "$collision_pid" 2>/dev/null || true
  exit 1
fi
if python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session collision-session --name james \
  --drafts --from-start >"$WORKDIR/collision-second.out" 2>"$WORKDIR/collision-second.err"; then
  echo "duplicate collision follower unexpectedly attached" >&2
  kill "$collision_pid" 2>/dev/null || true
  wait "$collision_pid" 2>/dev/null || true
  exit 1
fi
kill "$collision_pid" 2>/dev/null || true
wait "$collision_pid" 2>/dev/null || true
grep -q "active follower" "$WORKDIR/collision-second.err"
# Reattach after the provider process disappears, then close cleanly so active
# name discovery sees the durable cursor but not a phantom live agent.
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session collision-session --name james \
  --drafts --from-start >"$WORKDIR/collision-recovered.jsonl"

names="$(python3 "$DEMUX" --bridge-home "$BRIDGE_HOME" --active-names)"
python3 - "$names" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["schema"] == "codescribe.agent-bridge.active-names.v1", o
assert o["names"] == [], o
PY

echo "bus-demux: ok"
