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
    "source": "test_fixture",
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
assert o["source"] == "test_fixture", o
assert o["state_change_allowed"] is True, o
PY

got="$(run_once --all)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["audience"] == "*", o
PY

# Normal macOS follow is driven by a vnode event, not interval polling. The
# interval remains only as a portability/recovery fallback when kqueue is not
# available.
python3 - "$DEMUX" "$WORKDIR/event-trigger.jsonl" <<'PY'
import importlib.util
import select
import sys
import threading
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux_event_test", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

bus = Path(sys.argv[2])
bus.write_text("", encoding="utf-8")
trigger = module.BusEventTrigger(bus, fallback_interval=0.05)

def append_event():
    time.sleep(0.05)
    with bus.open("a", encoding="utf-8") as handle:
        handle.write("event\n")
        handle.flush()

writer = threading.Thread(target=append_event)
writer.start()
started = time.monotonic()
fired = trigger.wait(timeout=1.0)
elapsed = time.monotonic() - started
writer.join()

if hasattr(select, "kqueue"):
    assert trigger.mode == "kqueue-vnode", trigger.mode
    assert fired is True, "kqueue did not report the append"
    assert elapsed < 0.8, elapsed
else:
    assert trigger.mode == "interval-fallback", trigger.mode
trigger.close()
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
# written after that cursor, plus unacknowledged deliveries in original order.
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

# Pipe delivery without acknowledgment must survive process restart unchanged.
python3 "$DEMUX" --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-a --name james \
  --drafts --from-start >"$WORKDIR/unacknowledged.jsonl"
python3 - "$DEMUX" "$BUS" "$BRIDGE_HOME" "$first" "$WORKDIR/unacknowledged.jsonl" <<'PY'
import json, subprocess, sys
demux, bus, root, first_path, replay_path = sys.argv[1:]
first = [json.loads(line) for line in open(first_path)][1:]
replayed = [json.loads(line) for line in open(replay_path)][1:]
assert replayed == first, (first, replayed)
for row in first:
    args = ['python3', demux, '--bus', bus, '--bridge-home', root,
            '--provider', 'codex', '--session', 'codex-session-a', '--ack', row['delivery_id']]
    wrong = args.copy()
    wrong[wrong.index('--session') + 1] = 'another-session'
    assert subprocess.run(wrong, capture_output=True).returncode != 0
    subprocess.run(args, capture_output=True, check=True)
    subprocess.run(args, capture_output=True, check=True)  # repeat receipt is harmless
PY

seal "James, komenda po recovery." transcript_sealed 13
second="$WORKDIR/second.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-a --name changed \
  --drafts --from-start >"$second"
python3 - "$first" "$second" <<'PY'
import json, sys
first = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
second = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8")]
assert [row["kind"] for row in second] == ["attach", "seal"], second
assert second[0]["resumed"] is True, second[0]
assert second[0]["name"] == "james", second[0]
assert second[1]["audience"] == "james", second[1]
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

# Active-name discovery expires presence without erasing recovery state.
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
active = module.active_leases(Path(sys.argv[3]), 120)
assert {item["name"] for item in active} == {"iwo"}, active
assert stale.exists(), stale
lease.close()

# Discovery after a long outage must preserve the bound name and unread cursor.
saved = module.read_json(lease.path)
saved.update(heartbeat_unix=time.time() - 999, active=True, cursor=17)
module.atomic_json(lease.path, saved)
assert module.active_leases(Path(sys.argv[3]), 120) == []
restored = module.SessionLease(
    root=Path(sys.argv[3]), provider="codex", provider_session_id="active-session",
    name="different", bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
    follow_from_end=True,
)
assert restored.resumed and restored.cursor == 17, restored.attach_receipt()
assert restored.name == "iwo", restored.attach_receipt()
restored.close()

# A damaged state record is not a fresh session. Preserve its bytes and refuse
# attachment rather than skipping unread commands by starting at EOF.
for damaged in (b'{"cursor":', b'\xff\xfe', b'{}', b'[]'):
    restored.path.write_bytes(damaged)
    assert module.active_leases(Path(sys.argv[3]), 120) == []
    try:
        module.SessionLease(
            root=Path(sys.argv[3]), provider="codex", provider_session_id="active-session",
            name="iwo", bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
            follow_from_end=True,
        )
    except ValueError as error:
        assert "unreadable" in str(error) or "different provider" in str(error), error
    else:
        raise AssertionError("damaged recovery state was overwritten")
    assert restored.path.read_bytes() == damaged
module.atomic_json(restored.path, saved)

# Recovery positions are byte offsets, not values to coerce or clamp.
for invalid_cursor in (-1, True, 17.9, "17", None):
    invalid = dict(saved, cursor=invalid_cursor)
    module.atomic_json(restored.path, invalid)
    before = restored.path.read_bytes()
    try:
        module.SessionLease(
            root=Path(sys.argv[3]), provider="codex", provider_session_id="active-session",
            name="iwo", bus=Path(sys.argv[2]), requested_id=None, ttl_seconds=120,
            follow_from_end=True,
        )
    except ValueError as error:
        assert "cursor" in str(error), error
    else:
        raise AssertionError(f"invalid cursor accepted: {invalid_cursor!r}")
    assert restored.path.read_bytes() == before
module.atomic_json(restored.path, saved)

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

# Transcript envelopes without the canonical schema are not bus authority.
assert module.parse_line('{"status":"transcript_sealed","text":"James, stale"}') is None

# A truncated/replaced bus is a new authority epoch: resume at its current EOF,
# never replay byte zero under an old provider lease.
rotated = Path(sys.argv[3]) / "rotated.jsonl"
rotated.write_text('{"schema":"codescribe.transcript.v1"}\n', encoding="utf-8")
entries, cursor = module.iter_new_lines(rotated, 999)
assert entries == [], entries
assert cursor == rotated.stat().st_size, cursor
PY

# A provider disconnect after attach but before command delivery must leave the
# cursor before that command. Recovery is allowed to replay; loss is forbidden.
python3 - "$DEMUX" "$BUS" "$BRIDGE_HOME" <<'PY'
import argparse, importlib.util, json, sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux_broken_pipe", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

bus = Path(sys.argv[2])
bridge_home = Path(sys.argv[3]) / "broken-pipe"
bus.write_text(json.dumps({
    "schema": "codescribe.transcript.v1",
    "sequence": 91,
    "session_id": "broken-pipe-session",
    "mode": "raw",
    "utterance_id": "utterance-91",
    "emitted_at": "2026-08-22T00:00:00Z",
    "status": "transcript_sealed",
    "text": "James, command must survive disconnect.",
}) + "\n", encoding="utf-8")

class FailSecondWrite:
    def __init__(self):
        self.writes = 0
    def write(self, value):
        self.writes += 1
        if self.writes == 2:
            raise BrokenPipeError("provider disconnected")
        return len(value)
    def flush(self):
        return None

args = argparse.Namespace(
    bus=bus, name="james", all=False, become=False, follow=False, once=False,
    from_start=True, drafts=False, provider="codex", session="pipe-session",
    lease=None, bridge_home=bridge_home, lease_ttl=120.0, debug=False,
    interval=0.0,
)
original = sys.stdout
sys.stdout = FailSecondWrite()
try:
    try:
        module.run(args)
    except BrokenPipeError:
        pass
    else:
        raise AssertionError("command emit should observe the broken pipe")
finally:
    sys.stdout = original

leases = list((bridge_home / "leases").glob("*.json"))
assert len(leases) == 1, leases
receipt = json.loads(leases[0].read_text(encoding="utf-8"))
assert receipt["cursor"] == 0, receipt
assert receipt["last_sequence"] is None, receipt
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


# --- Evidence schema: the app's real lane since 2026-08-27 22:36 ------------
# A full reducer snapshot is payload, not bridge control input. These rows
# prove exact forwarding, metadata-only seal coalescing, and draft authority.
evidence() {
  local rendered="$1" action="${2:-apply_ledger_decision}" sequence="${3:-1}"
  local revision="${4:-$sequence}"
  python3 - "$BUS" "$rendered" "$action" "$sequence" "$revision" <<'PY'
import json, sys
path, rendered, action, sequence, revision = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": sequence,
        "session_id": "evidence-session",
        "mode": "dictation",
        "reducer_action": action,
        "reducer_revision": revision,
        "rendered_text": rendered,
        "emitted_at": "2026-08-28T15:00:00Z",
    }, ensure_ascii=False) + "\n")
PY
}

: >"$BUS"
evidence "James sprawdź"            apply_ledger_decision 1
evidence "James sprawdź plik"       apply_ledger_decision 2
evidence "James sprawdź plik alfa"  apply_ledger_decision 3
evidence "James sprawdź plik beta"  apply_ledger_decision 4
evidence "James sprawdź plik beta"  record_ledger_terminal_seal 5 5
evidence "James sprawdź plik beta"  record_ledger_terminal_seal 6 5
evidence "James sprawdź plik beta"  record_ledger_terminal_seal 7 5

ev="$WORKDIR/evidence.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-ev --name james \
  --drafts --from-start >"$ev"
python3 - "$ev" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
kinds = [row["kind"] for row in rows]

# 1. Heard at all. A follower filtering on the clean schema emits only `attach`.
assert len(kinds) > 1, f"deaf to the evidence schema: {kinds}"

# 2. One seal, though the reducer wrote three rows for the same terminal phase.
assert kinds == ["attach", "revised", "revised", "revised", "revised", "seal"], kinds

texts = [row["text"] for row in rows[1:]]
# 3. Every live event preserves its exact full reducer snapshot. In particular,
# the unrelated `beta` revision is not guessed as a suffix.
assert texts[:4] == [
    "James sprawdź",
    "James sprawdź plik",
    "James sprawdź plik alfa",
    "James sprawdź plik beta",
], texts

seal = rows[-1]
assert seal["text"] == "James sprawdź plik beta", seal
assert seal["state_change_allowed"] is True, seal
assert all(row["state_change_allowed"] is False for row in rows[1:-1]), rows

assert all(row["producer_schema"] == "codescribe.transcript-evidence.v1" for row in rows[1:]), rows
assert all(row["source_event_id"] for row in rows[1:]), rows
PY

# Identity is metadata-only: identical payloads from distinct observations are
# distinct, while a terminal phase is coalesced by its reducer identity. Two
# named provider leases remain independent and namespace their delivery IDs.
python3 - "$DEMUX" "$BUS" "$BRIDGE_HOME" <<'PY'
import importlib.util, sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux_identity", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)

def evidence(sequence, revision, action="apply_ledger_decision", sample_start=0):
    return {
        "schema": module.EVIDENCE_SCHEMA,
        "session_id": "identity-session",
        "sequence": sequence,
        "reducer_revision": revision,
        "reducer_action": action,
        "occurrence_session_id": "identity-session",
        "capture_epoch": 3,
        "sample_start": sample_start,
        "sample_end": sample_start + 100,
        "document_index": 0,
        "rendered_text": "Lumen and Kimi: identical words are valid twice.",
    }

normalizer = module.EvidenceNormalizer()
first = normalizer.normalize(evidence(101, 1, sample_start=10))
second = normalizer.normalize(evidence(102, 2, sample_start=110))
assert first and second
assert first["text"] == second["text"], (first, second)
assert first["source_event_id"] != second["source_event_id"], (first, second)
assert first["status"] == second["status"] == "utterance_revised", (first, second)

seal_a = normalizer.normalize(evidence(103, 3, module.TERMINAL_SEAL, 10))
seal_b = normalizer.normalize(evidence(104, 3, module.TERMINAL_SEAL, 110))
assert seal_a and seal_a["status"] == module.SEALED, seal_a
assert seal_b is None, seal_b

root = Path(sys.argv[3]) / "lease-coexistence"
bus = Path(sys.argv[2])
lumen = module.SessionLease(
    root=root, provider="codex", provider_session_id="lumen-thread", name="lumen",
    bus=bus, requested_id=None, ttl_seconds=120, follow_from_end=True,
)
kimi = module.SessionLease(
    root=root, provider="cursor", provider_session_id="kimi-thread", name="kimi",
    bus=bus, requested_id=None, ttl_seconds=120, follow_from_end=True,
)
try:
    assert lumen.lease_id != kimi.lease_id
    assert {row["name"] for row in module.active_leases(root, 120)} == {"lumen", "kimi"}
    try:
        module.SessionLease(
            root=root, provider="codex", provider_session_id="lumen-thread", name="lumen",
            bus=bus, requested_id="different-lease-id", ttl_seconds=120, follow_from_end=True,
        )
    except ValueError as error:
        assert "does not belong" in str(error)
    else:
        raise AssertionError("one provider session must not fork a second lease")
    lumen_payload = module.slim(first, "lumen")
    kimi_payload = module.slim(first, "kimi")
    lumen.enrich(lumen_payload)
    kimi.enrich(kimi_payload)
    assert lumen_payload["delivery_id"] != kimi_payload["delivery_id"]
    assert lumen_payload["delivery_owner"]["provider"] == "codex"
    assert kimi_payload["delivery_owner"]["provider"] == "cursor"
    replay = module.slim(first, "lumen")
    lumen.enrich(replay)
    assert replay["delivery_id"] == lumen_payload["delivery_id"]
    # Restart between terminal projection rows loses the in-memory coalescer,
    # but must not manufacture a different command delivery identity.
    restarted_seal = module.EvidenceNormalizer().normalize(
        evidence(104, 3, module.TERMINAL_SEAL, 110)
    )
    seal_delivery = module.slim(seal_a, "lumen")
    restarted_delivery = module.slim(restarted_seal, "lumen")
    lumen.enrich(seal_delivery)
    lumen.enrich(restarted_delivery)
    assert seal_delivery["source_event_id"] != restarted_delivery["source_event_id"]
    assert seal_delivery["delivery_id"] == restarted_delivery["delivery_id"]
    next_seal = module.EvidenceNormalizer().normalize(
        evidence(105, 4, module.TERMINAL_SEAL, 110)
    )
    next_delivery = module.slim(next_seal, "lumen")
    lumen.enrich(next_delivery)
    assert next_delivery["delivery_id"] != seal_delivery["delivery_id"]
    assigned_delivery = module.slim(seal_a, "lumen", kind="name_assignment")
    assigned_replay = module.slim(restarted_seal, "lumen", kind="name_assignment")
    lumen.enrich(assigned_delivery)
    lumen.enrich(assigned_replay)
    assert assigned_delivery["delivery_id"] == assigned_replay["delivery_id"]
finally:
    kimi.close()
    lumen.close()
PY

# Per-take wav identity: demux assigns ~/.codescribe/sessions/<session_id>.wav
# (or $CODESCRIBE_DATA_DIR/sessions/...). last_session.wav is never the id.
WAV_HOME="$WORKDIR/codescribe-home"
mkdir -p "$WAV_HOME/sessions"
printf 'session-take' >"$WAV_HOME/sessions/test-session.wav"
printf 'stale-alias' >"$WAV_HOME/last_session.wav"
: >"$BUS"
CODESCRIBE_DATA_DIR="$WAV_HOME" seal "James, ten take ma własne audio."
got="$(CODESCRIBE_DATA_DIR="$WAV_HOME" run_once --name james)"
python3 - "$got" "$WAV_HOME" <<'PY'
import json, sys
from pathlib import Path
o = json.loads(sys.argv[1])
home = Path(sys.argv[2]).resolve()
assigned = Path(o["wav"]).resolve()
assert assigned == home / "sessions" / "test-session.wav", o
assert assigned.name != "last_session.wav", o
assert "last_session.wav" not in o["wav"], o
PY

python3 - "$DEMUX" "$WAV_HOME" <<'PY'
import importlib.util, os, sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
home = Path(sys.argv[2]).resolve()
env = {"CODESCRIBE_DATA_DIR": str(home)}
event = {
    "session_id": "test-session",
    "wav": str(home / "last_session.wav"),
}
wav = module.assigned_session_wav(event, env)
assert Path(wav).name != module.LAST_SESSION_WAV, wav
assert Path(wav).resolve() == home / "sessions" / "test-session.wav", wav
assert module.assigned_session_wav({"session_id": "../etc"}, env) is None
assert module.assigned_session_wav({"session_id": "short"}, env) is None
PY

# One-mic idle: abandoned historical starts must not poison install.
python3 - "$DEMUX" "$WORKDIR" <<'PY'
import importlib.util, json, sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
root = Path(sys.argv[2])

def write(name, rows):
    path = root / name
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    return path

live = write("idle-live.jsonl", [{"session_id": "live", "status": "session_started"}])
assert module.installation_idle(live) is False

closed = write(
    "idle-closed.jsonl",
    [
        {"session_id": "closed", "status": "session_started"},
        {"session_id": "closed", "status": "session_ended"},
    ],
)
assert module.installation_idle(closed) is True

abandoned = write(
    "idle-abandoned.jsonl",
    [
        {"session_id": "old-a", "status": "session_started"},
        {"session_id": "old-b", "status": "session_started"},
        {"session_id": "now", "status": "session_started"},
        {"session_id": "now", "status": "session_ended"},
    ],
)
assert module.installation_idle(abandoned) is True

nested_cli = write(
    "idle-nested-cli.jsonl",
    [
        {"session_id": "app", "status": "session_started"},
        {
            "session_id": "cli",
            "status": "session_started",
            "source": module.CLI_FILE_VERDICT_SOURCE,
        },
        {
            "session_id": "cli",
            "status": "session_ended",
            "source": module.CLI_FILE_VERDICT_SOURCE,
        },
    ],
)
assert module.installation_idle(nested_cli) is False

cli_live = write(
    "idle-cli-live.jsonl",
    [
        {
            "session_id": "cli-open",
            "status": "session_started",
            "source": module.CLI_FILE_VERDICT_SOURCE,
        }
    ],
)
assert module.installation_idle(cli_live) is False

# A crashed CLI run (e.g. SIGPIPE mid-transcription) leaves an unpaired start
# forever. Provably old = abandoned; fresh or timestamp-less = still live.
import datetime

utcnow = datetime.datetime.now(datetime.timezone.utc)
old_ts = (utcnow - datetime.timedelta(hours=48)).isoformat().replace("+00:00", "Z")
fresh_ts = utcnow.isoformat().replace("+00:00", "Z")

cli_abandoned = write(
    "idle-cli-abandoned.jsonl",
    [
        {
            "session_id": "cli-crashed",
            "status": "session_started",
            "source": module.CLI_FILE_VERDICT_SOURCE,
            "emitted_at": old_ts,
        }
    ],
)
assert module.installation_idle(cli_abandoned) is True

cli_fresh = write(
    "idle-cli-fresh.jsonl",
    [
        {
            "session_id": "cli-running",
            "status": "session_started",
            "source": module.CLI_FILE_VERDICT_SOURCE,
            "emitted_at": fresh_ts,
        }
    ],
)
assert module.installation_idle(cli_fresh) is False

cli_bad_ts = write(
    "idle-cli-bad-ts.jsonl",
    [
        {
            "session_id": "cli-mystery",
            "status": "session_started",
            "source": module.CLI_FILE_VERDICT_SOURCE,
            "emitted_at": "not-a-timestamp",
        }
    ],
)
assert module.installation_idle(cli_bad_ts) is False
PY

# Acknowledgment works while the sole follower owns its lock. Capacity refusal
# retains pending deliveries and leaves the next event unread for recovery.
python3 - "$DEMUX" "$WORKDIR" <<'PY'
import json, subprocess, sys, time
from pathlib import Path
demux, directory = sys.argv[1:]
root = Path(directory) / 'ack-process'
root.mkdir()
bus = root / 'bus.jsonl'
events = [dict(schema='codescribe.transcript.v1', sequence=i,
               session_id='ack-process', utterance_id=str(i),
               status='transcript_sealed', text='Roman, test fixture.')
          for i in range(257)]
bus.write_text(''.join(json.dumps(row)+'\n' for row in events))
base = ['python3', demux, '--bus', str(bus), '--bridge-home', str(root),
        '--provider', 'test', '--session', 'capacity', '--name', 'Roman']
full = subprocess.run(base+['--from-start'], capture_output=True, text=True)
assert full.returncode == 4, full.stderr
state_path = next((root/'leases').glob('*.json'))
state = json.loads(state_path.read_text())
assert len(state['pending']) == 256 and state['cursor'] < bus.stat().st_size
ack = base[:base.index('--name')]
for payload in state['pending']:
    subprocess.run(ack+['--ack', payload['delivery_id']], capture_output=True, check=True)
recovered = subprocess.run(base+['--from-start'], capture_output=True, text=True, check=True)
rows = [json.loads(line) for line in recovered.stdout.splitlines()]
assert [row['sequence'] for row in rows[1:]] == [256], rows

follower = subprocess.Popen(base+['--follow'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
try:
    deadline = time.monotonic()+5
    while True:
        state = json.loads(state_path.read_text())
        if state['active'] and state['pid'] == follower.pid:
            break
        assert follower.poll() is None and time.monotonic() < deadline
        time.sleep(.02)
    subprocess.run(ack+['--ack', state['pending'][0]['delivery_id']],
                   capture_output=True, check=True)
    while json.loads(state_path.read_text())['pending']:
        assert follower.poll() is None and time.monotonic() < deadline
        time.sleep(.02)
finally:
    follower.terminate()
    follower.communicate(timeout=5)

# Keep the same live process at capacity; acknowledge one item and verify the
# blocked evidence seal is delivered, not consumed by normalization on retry.
events[-1] = dict(schema='codescribe.transcript-evidence.v1', sequence=256,
                  session_id='blocked-evidence', reducer_revision=1,
                  reducer_action='record_ledger_terminal_seal',
                  rendered_text='Roman, blocked terminal fixture.')
bus.write_text(''.join(json.dumps(row)+'\n' for row in events))
continuous = base.copy()
continuous[continuous.index('--session')+1] = 'continuous'
import hashlib
identifier = hashlib.sha256(b'test\0continuous').hexdigest()[:32]
continuous_state = root/'leases'/f'{identifier}.json'
with (root/'continuous.jsonl').open('w') as output:
    follower = subprocess.Popen(continuous+['--follow', '--from-start'],
                                stdout=output, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic()+15
        while True:
            state = json.loads(continuous_state.read_text()) if continuous_state.exists() else {}
            if len(state.get('pending', [])) == 256:
                break
            assert follower.poll() is None and time.monotonic() < deadline
            time.sleep(.02)
        heartbeat = state['heartbeat_unix']
        time.sleep(1.2)
        state = json.loads(continuous_state.read_text())
        assert follower.poll() is None and state['heartbeat_unix'] > heartbeat
        assert state['cursor'] < bus.stat().st_size
        ack = continuous[:continuous.index('--name')]
        subprocess.run(ack+['--ack', state['pending'][0]['delivery_id']],
                       capture_output=True, check=True)
        while True:
            state = json.loads(continuous_state.read_text())
            if state['cursor'] == bus.stat().st_size:
                break
            assert follower.poll() is None and time.monotonic() < deadline
            time.sleep(.02)
        assert state['pending'][-1]['text'] == 'Roman, blocked terminal fixture.'
        assert state['pending'][-1]['state_change_allowed'] is True
        assert state['pid'] == follower.pid
    finally:
        follower.terminate()
        follower.communicate(timeout=5)
rows = [json.loads(line) for line in (root/'continuous.jsonl').read_text().splitlines()]
assert len(rows) == 258, len(rows)  # attach + 257 deliveries, no retry duplicate
assert rows[-1]['text'] == 'Roman, blocked terminal fixture.'
PY

python3 - "$DEMUX" "$WORKDIR" <<'PY'
import importlib.util, json, subprocess, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location('bus_names', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for text in ('Raman, sprawdź.', 'Rmoan, sprawdź.', 'Rman, sprawdź.', 'Romaan, sprawdź.', 'Hej Raman, sprawdź.'):
    assert module.resolve_recipients(text, {'roman'}) == ({'roman'}, 'fuzzy'), text
assert module.resolve_recipients('ROMANIE, sprawdź.', {'roman', 'ramon'}) == ({'roman'}, 'exact')
assert module.resolve_recipients('Raman, sprawdź.', {'roman', 'ramon'}) == ({'roman', 'ramon'}, 'ambiguous')
assert module.resolve_recipients('Raman, sprawdź.', {'roman', 'raman'}) == ({'raman'}, 'exact')
assert module.resolve_recipients('To jest Raman.', {'roman'}) == (set(), 'none')
assert module.resolve_recipients('Iva, sprawdź.', {'iwo'}) == (set(), 'none')

root = Path(sys.argv[2])/'recipient-process'
bus = root/'bus.jsonl'
root.mkdir()
bus.touch()
def register(name):
    lease = module.SessionLease(root=root, provider='test', provider_session_id=name,
                                name=name, bus=bus, requested_id=None,
                                ttl_seconds=120, follow_from_end=False)
    lease.close()  # offline identity must still prevent misrouting
register('roman')
other_bus = root/'other-bus.jsonl'
other_bus.touch()
other = module.SessionLease(root=root, provider='test', provider_session_id='other-bus',
                            name='ramon', bus=other_bus, requested_id=None,
                            ttl_seconds=120, follow_from_end=False)
other.close()
assert module.registered_recipients(root, bus) == {'roman'}
event = dict(schema=module.CLEAN_SCHEMA, session_id='names', sequence=1,
             status=module.SEALED, text='Raman, sprawdź.')
bus.write_text(json.dumps(event)+'\n')
cmd = ['python3', sys.argv[1], '--bus', str(bus), '--bridge-home', str(root),
       '--name', 'Roman', '--once']
row = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)
assert row['routing_match'] == 'fuzzy' and row['text'] == event['text']
assert row['state_change_allowed'] is True
event['status'] = 'utterance_draft'
bus.write_text(json.dumps(event)+'\n')
row = json.loads(subprocess.run(cmd+['--drafts'], capture_output=True, text=True, check=True).stdout)
assert row['routing_match'] == 'fuzzy' and row['state_change_allowed'] is False
event['status'] = module.SEALED
bus.write_text(json.dumps(event)+'\n')
register('ramon')
row = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)
assert row['kind'] == 'routing_ambiguity' and row['state_change_allowed'] is False
assert row['routing_candidates'] == ['ramon', 'roman'] and row['text'] == event['text']
register('raman')
assert subprocess.run(cmd, capture_output=True).returncode == 1
(root/'leases'/'damaged.json').write_text('{')
assert module.registered_recipients(root, bus) is None
assert subprocess.run(cmd, capture_output=True).returncode == 1
event['text'] = 'Roman, sprawdź.'
bus.write_text(json.dumps(event)+'\n')
assert subprocess.run(cmd, capture_output=True).returncode == 0
PY

echo "bus-demux: ok"
