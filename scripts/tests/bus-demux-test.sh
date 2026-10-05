#!/usr/bin/env bash
# Hermetic kielbasa checks for scripts/bus-demux.py. No microphone.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DEMUX="$ROOT/scripts/bus-demux.py"
WORKDIR="$(mktemp -d)"
# Install guards include channel buses; fixtures must never inspect the user bridge.
export CODESCRIBE_AGENT_BRIDGE_HOME="$WORKDIR/ambient-agent-bridge"
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
    interval=0.0, coalesce=False, on_seal=None,
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

# Coverage-refused safety net: a channel session whose ledger never issued a
# terminal seal (incomplete acoustic coverage / pending recovery) must still
# deliver its words when the channel moves on — as a seal envelope with
# coverage="refused" and state_change_allowed=false. A session that DID seal
# terminally must not be re-delivered by the net.
: >"$BUS"
python3 - "$BUS" <<'PY'
import hashlib, json, sys
rows = [
    # Lost session: drafts only, ledger refused the terminal seal.
    {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 1,
        "session_id": "agent-channel-2-lost",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": "apply_ledger_decision",
        "reducer_revision": 1,
        "document_index": 0,
        "rendered_text": "James, ta wypowiedź nie dostała terminal seala.",
        "emitted_at": "2026-09-29T15:55:30Z",
    },
    {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 2,
        "session_id": "agent-channel-2-lost",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": "seal_coverage",
        "reducer_revision": 2,
        "document_index": 0,
        "rendered_text": "James, ta wypowiedź nie dostała terminal seala. Cała.",
        "emitted_at": "2026-09-29T15:55:52Z",
    },
    # The reducer re-projects the same document identity. Its duplicate
    # observation must not create a second occurrence.
    {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 3,
        "session_id": "agent-channel-2-lost",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": "seal_coverage",
        "reducer_revision": 2,
        "document_index": 0,
        "rendered_text": "James, ta wypowiedź nie dostała terminal seala. Cała.",
        "emitted_at": "2026-09-29T15:55:52Z",
    },
    # Healthy session: draft then a real terminal seal.
    {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 4,
        "session_id": "agent-channel-2-good",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": "apply_ledger_decision",
        "reducer_revision": 1,
        "document_index": 0,
        "rendered_text": "James, jesteś tam?",
        "emitted_at": "2026-09-29T15:56:50Z",
    },
    {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": 5,
        "session_id": "agent-channel-2-good",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": "record_ledger_terminal_seal",
        "reducer_revision": 2,
        "document_index": 0,
        "rendered_text": "James, jesteś tam?",
        "emitted_at": "2026-09-29T15:56:55Z",
    },
    # The channel reopens: the flush boundary for both earlier sessions.
    {
        "schema": "codescribe.channel-session.v1",
        "kind": "channel_session",
        "state": "open",
        "reason": "opened",
        "channel": "2",
        "agent": "james",
        "session_id": "agent-channel-2-next",
        "emitted_at": "2026-09-29T15:57:28Z",
    },
]
bus = str(__import__("pathlib").Path(sys.argv[1]).resolve())
session = "codex-session-refused"
owner = {"provider": "codex", "provider_session_id": session,
         "lease_id": hashlib.sha256(("codex\0" + session).encode()).hexdigest()[:32],
         "bus": bus, "channel": "2", "name": "james"}
opening = lambda capture, time: {
    "schema": "codescribe.channel-session.v1", "kind": "channel_session",
    "state": "open", "channel": "2", "agent": "james", "session_id": capture,
    "opened_at": time, "emitted_at": time}
rows.insert(0, opening("agent-channel-2-lost", "2026-09-29T15:55:00Z"))
rows.insert(4, opening("agent-channel-2-good", "2026-09-29T15:56:00Z"))
rows[-1]["opened_at"] = rows[-1]["emitted_at"]
with open(sys.argv[1], "a", encoding="utf-8") as handle:
    for row in rows:
        row["recipients"] = [owner]
        handle.write(json.dumps(row, ensure_ascii=False) + "\n")
PY
refused="$WORKDIR/coverage-refused.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-refused --name james \
  --drafts --from-start >"$refused"
python3 - "$refused" "$DEMUX" <<'PY'
import importlib.util, json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
kinds = [row["kind"] for row in rows]
# The newer opening closes the earlier unsealed document before the next
# draft. Reobserving the same document does not create another occurrence.
assert kinds == ["attach", "revised", "revised", "seal", "revised", "revised", "seal"], kinds
healthy = rows[6]
assert healthy["session_id"] == "agent-channel-2-good", healthy
assert healthy.get("coverage") is None, healthy
assert healthy["state_change_allowed"] is True, healthy
flushed = rows[3]
assert flushed["session_id"] == "agent-channel-2-lost", flushed
assert flushed["status"] == "transcript_sealed", flushed
assert flushed["coverage"] == "refused", flushed
assert flushed["state_change_allowed"] is False, flushed
assert flushed["text"].endswith("Cała."), flushed
# The healthy session must not be re-delivered by the net: exactly one
# envelope carries coverage=refused and it names the lost session only.
assert [row["session_id"] for row in rows if row.get("coverage") == "refused"] == [
    "agent-channel-2-lost"
], rows

# Restart parity: the refused phase keys one delivery identity across a bus
# replay, exactly like a terminal seal phase.
spec = importlib.util.spec_from_file_location("bus_demux_refused", sys.argv[2])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
identity = module.channel_message_identity("agent-channel-2-lost")
assert flushed["message_id"] == identity, flushed
phase = module._identity(("channel-message-seal", "agent-channel-2-lost"))
assert flushed["delivery_id"] == module._identity((
    "native_bus_demux", flushed["lease_id"], phase, "seal", "james")), flushed
assert flushed["reducer_revision"] == 2, flushed
assert flushed["reducer_action"] == "seal_coverage", flushed
PY

# Hang-up net: a channel closed by a second digit / Fn press is never
# reopened, so no later channel-session row releases its refused take. The
# session's own `session_ended` lifecycle row is the flush boundary. Late rows
# after the hang-up and later channel receipts must not deliver it twice.
: >"$BUS"
python3 - "$BUS" <<'PY'
import hashlib, json, sys
coverage = {
    "status": "incomplete",
    "speech_samples": 463872,
    "covered_samples": 454656,
    "coverage_ratio": 0.98,
}
def evidence(sequence, action, revision, doc, text, **extra):
    row = {
        "schema": "codescribe.transcript-evidence.v1",
        "sequence": sequence,
        "session_id": "agent-channel-4-hangup",
        "audience": "james",
        "mode": "dictation",
        "reducer_action": action,
        "reducer_revision": revision,
        "document_index": doc,
        "rendered_text": text,
        "phase": "listening",
        "terminal": False,
        "lifecycle_terminal": False,
        "emitted_at": f"2026-09-29T16:34:{sequence:02d}Z",
    }
    row.update(extra)
    return row
full = "James, rozłączam się przed silence sealem. Całość."
rows = [
    evidence(1, "apply_ledger_decision", 1, 0, "James, rozłączam się"),
    evidence(2, "apply_ledger_decision", 3, 0, "przed silence sealem."),
    evidence(3, "seal_coverage", 20, 0, full, seal_coverage=coverage),
    evidence(4, "seal_coverage", 20, 0, full, seal_coverage=coverage),
    {
        "schema": "codescribe.transcript.v1",
        "sequence": 5,
        "session_id": "agent-channel-4-hangup",
        "mode": "dictation",
        "utterance_id": None,
        "status": "session_ended",
        "terminal": True,
        "end_reason": "coverage_refused",
        "emitted_at": "2026-09-29T16:35:00Z",
    },
    # A late re-projection after the hang-up must not re-arm the net.
    evidence(6, "seal_coverage", 21, 0, full, seal_coverage=coverage),
    # Neither a closing receipt for the same session nor a fresh open of the
    # channel may deliver the refused take a second time.
    {
        "schema": "codescribe.channel-session.v1",
        "kind": "channel_session",
        "state": "sealed",
        "reason": "silence",
        "channel": "4",
        "agent": "james",
        "session_id": "agent-channel-4-hangup",
        "emitted_at": "2026-09-29T16:35:02Z",
    },
    {
        "schema": "codescribe.channel-session.v1",
        "kind": "channel_session",
        "state": "open",
        "reason": "opened",
        "channel": "4",
        "agent": "james",
        "session_id": "agent-channel-4-next",
        "emitted_at": "2026-09-29T16:40:00Z",
    },
]
bus = str(__import__("pathlib").Path(sys.argv[1]).resolve())
session = "codex-session-hangup"
owner = {"provider": "codex", "provider_session_id": session,
         "lease_id": hashlib.sha256(("codex\0" + session).encode()).hexdigest()[:32],
         "bus": bus, "channel": "4", "name": "james"}
with open(sys.argv[1], "a", encoding="utf-8") as handle:
    for row in rows:
        row["recipients"] = [owner]
        if row.get("state") == "open":
            row["opened_at"] = row["emitted_at"]
        handle.write(json.dumps(row, ensure_ascii=False) + "\n")
PY
hangup="$WORKDIR/coverage-refused-hangup.jsonl"
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$BRIDGE_HOME" \
  --provider codex --session codex-session-hangup --name james \
  --drafts --from-start >"$hangup"
python3 - "$hangup" "$DEMUX" "$BUS" <<'PY'
import importlib.util, json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
refused = [row for row in rows if row.get("coverage") == "refused"]
assert len(refused) == 1, [row["kind"] for row in rows]
flushed = refused[0]
assert flushed["kind"] == "seal", flushed
assert flushed["session_id"] == "agent-channel-4-hangup", flushed
assert flushed["state_change_allowed"] is False, flushed
assert flushed["text"].endswith("Całość."), flushed
# The hang-up row itself released the take, not a later channel receipt.
assert flushed["emitted_at"] == "2026-09-29T16:35:00Z", flushed
assert not [row for row in rows if row.get("state_change_allowed") is True], rows

spec = importlib.util.spec_from_file_location("bus_demux_hangup", sys.argv[2])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
# The lease mailbox would mask a second flush behind the same delivery id, so
# prove single delivery at the normalizer itself, as a lease-less reader sees it.
normalizer = module.EvidenceNormalizer()
flushed_at = []
for line in open(sys.argv[3], encoding="utf-8"):
    normalizer.normalize(module.parse_line(line))
    flushed_at.extend(row["emitted_at"] for row in normalizer.pop_flushes())
assert flushed_at == ["2026-09-29T16:35:00Z"], flushed_at
# A dictation take's session_ended passes through untouched and flushes
# nothing: the net only holds addressed channel sessions.
normalizer = module.EvidenceNormalizer()
ended = {"schema": module.CLEAN_SCHEMA, "session_id": "dictation-take", "status": "session_ended"}
assert normalizer.normalize(ended) is ended
assert normalizer.pop_flushes() == []
PY

# A take's audio is its own ~/.codescribe/sessions/<session_id>.wav
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

# --- Audience field routing (FN-1) -------------------------------------------
sealed_with_audience() {
  local text="$1"
  local audience="$2"
  local status="${3:-transcript_sealed}"
  local sequence="${4:-1}"
  python3 - "$BUS" "$text" "$audience" "$status" "$sequence" <<'PY'
import json, sys
path, text, audience, status, sequence = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
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
    "audience": audience,
}
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(event, ensure_ascii=False) + "\n")
PY
}

: >"$BUS"
sealed_with_audience "neutral text without any name" "james" transcript_sealed 100
got="$(run_once --name james)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["audience"] == "james", o
assert o["routing_match"] == "audience", o
assert "neutral text" in o["text"], o
assert o["state_change_allowed"] is True, o
PY

if run_once --name leon >/dev/null 2>/dev/null; then
  echo "expected audience-bound row to drop for leon" >&2
  exit 1
fi

: >"$BUS"
sealed_with_audience "James, wklejka nadal parkuje." "leon" transcript_sealed 101
got="$(run_once --name james)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["audience"] == "james", o
assert o.get("routing_match") != "audience", o
assert "James" in o["text"], o
assert o["state_change_allowed"] is True, o
PY

: >"$BUS"
sealed_with_audience "broadcast neutral text" "*" transcript_sealed 102
got_james="$(run_once --name james)"
got_leon="$(run_once --name leon)"
python3 - "$got_james" "$got_leon" <<'PY'
import json, sys
james = json.loads(sys.argv[1])
leon = json.loads(sys.argv[2])
assert james["audience"] == "*", james
assert leon["audience"] == "*", leon
assert james["routing_match"] == "audience", james
assert leon["routing_match"] == "audience", leon
assert james["text"] == leon["text"], (james, leon)
PY

: >"$BUS"
python3 - "$BUS" <<'PY'
import json, sys
path = sys.argv[1]
event = {
    "schema": "codescribe.transcript.v1",
    "sequence": 103,
    "session_id": "test-session",
    "mode": "raw",
    "utterance_id": "utterance-1",
    "emitted_at": "2026-08-20T22:00:00Z",
    "status": "transcript_sealed",
    "text": "future field tolerance",
    "source": "test_fixture",
    "audience": "james",
    "future_field": {"nested": [1, 2, 3]},
}
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(event, ensure_ascii=False) + "\n")
PY
got="$(run_once --name james)"
python3 - "$got" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["routing_match"] == "audience", o
assert o["text"] == "future field tolerance", o
assert o["audience"] == "james", o
PY

# ── One-command channel engine (#74 W1): coalesce, on-seal, status, attach ──

# Coalescing folds a revision storm into one live envelope per document while
# the seal stays a separate envelope; the newest revision wins.
COAL_HOME="$WORKDIR/coalesce-bridge"
: >"$BUS"
for i in $(seq 20 69); do
  seal "James, rewizja numer $i." utterance_revised "$i"
done
seal "James, dokument zamknięty." transcript_sealed 70
python3 "$DEMUX" \
  --bus "$BUS" --bridge-home "$COAL_HOME" \
  --provider codex --session codex-coalesce --name james \
  --drafts --from-start --coalesce >/dev/null
python3 - "$COAL_HOME" <<'PY'
import json, sys
from pathlib import Path
leases = list((Path(sys.argv[1]) / "leases").glob("*.json"))
assert len(leases) == 1, leases
state = json.loads(leases[0].read_text(encoding="utf-8"))
kinds = sorted(item["kind"] for item in state["pending"])
assert kinds == ["revised", "seal"], kinds
revised = next(item for item in state["pending"] if item["kind"] == "revised")
assert "numer 69" in revised["text"], revised["text"]
PY

# Status reads the marker store: the backlog is pending minus markers, never
# the raw pending length the lease file keeps until the follower's sweep.
status_line="$(python3 "$DEMUX" --bus "$BUS" --bridge-home "$COAL_HOME" \
  --provider codex --session codex-coalesce --status)"
python3 - "$status_line" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["attached"] is True, o
assert o["pending_file"] == 2, o
assert o["backlog"] == 2, o
assert o["unacked_seals"] == 1, o
assert o["last_seal"] and "zamkni" in o["last_seal"]["text"], o
assert o["follower_alive"] is False, o
PY
seal_id="$(python3 - "$COAL_HOME" <<'PY'
import json, sys
from pathlib import Path
lease = next((Path(sys.argv[1]) / "leases").glob("*.json"))
state = json.loads(lease.read_text(encoding="utf-8"))
print(next(i["delivery_id"] for i in state["pending"] if i["kind"] == "seal"))
PY
)"
python3 "$DEMUX" --bus "$BUS" --bridge-home "$COAL_HOME" \
  --provider codex --session codex-coalesce --ack "$seal_id" >/dev/null
status_line="$(python3 "$DEMUX" --bus "$BUS" --bridge-home "$COAL_HOME" \
  --provider codex --session codex-coalesce --status)"
python3 - "$status_line" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["pending_file"] == 2, o
assert o["acknowledged_markers"] == 1, o
assert o["backlog"] == 1, o
assert o["unacked_seals"] == 0, o
PY

# The on-seal hook fires exactly once per freshly queued seal: a storm of
# revisions stays silent, and a rerun replaying the pending seal on the same
# lease does not re-fire it.
HOOK_HOME="$WORKDIR/hook-bridge"
HOOK_LOG="$WORKDIR/hook.log"
: >"$BUS"
: >"$HOOK_LOG"
for i in $(seq 20 29); do
  seal "James, rewizja $i." utterance_revised "$i"
done
seal "James, pierwszy seal." transcript_sealed 30
run_hooked() {
  python3 "$DEMUX" \
    --bus "$BUS" --bridge-home "$HOOK_HOME" \
    --provider codex --session codex-hook --name james \
    --drafts --from-start --coalesce \
    --on-seal "printf '%s\\n' \"\$CODESCRIBE_SEAL_DELIVERY_ID\" >>$HOOK_LOG" \
    >/dev/null
}
run_hooked
for _ in $(seq 1 30); do
  test -s "$HOOK_LOG" && break
  sleep 0.1
done
test "$(wc -l <"$HOOK_LOG" | tr -d ' ')" = "1"
run_hooked
sleep 0.3
test "$(wc -l <"$HOOK_LOG" | tr -d ' ')" = "1"

# Attach is the one command from zero to a listening channel: it binds the
# channel, spawns exactly one coalescing follower, and prints a receipt with
# the voice profile. A second attach reuses the live follower.
ATTACH_HOME="$WORKDIR/attach-bridge"
: >"$BUS"
mkdir -p "$ATTACH_HOME"
python3 - "$ATTACH_HOME" <<'PY'
import json, sys
from pathlib import Path
Path(sys.argv[1], "voices.json").write_text(
    json.dumps(
        {
            "schema": "codescribe.agent-voice-binding.v1",
            "bindings": {"james": "sal"},
            "profiles": {"james": {"voice": "sal", "speed": 1.1}},
        }
    ),
    encoding="utf-8",
)
PY
receipt="$(python3 "$DEMUX" --bus "$BUS" --bridge-home "$ATTACH_HOME" \
  --provider codex --session codex-attach --name james \
  --attach --channel 3)"
follower_pid="$(python3 - "$receipt" "$ATTACH_HOME" <<'PY'
import json, sys
from pathlib import Path
o = json.loads(sys.argv[1])
assert o["kind"] == "attach_receipt", o
assert o["channel"] == "3", o
assert o["audience"] == "james", o
assert o["follower_spawned"] is True, o
assert o["voice"]["voice"] == "sal", o
assert o["voice"]["speed"] == 1.1, o
binding = json.loads(
    Path(sys.argv[2], "vc.agent-audience-binding.v1.json").read_text(encoding="utf-8")
)
assert binding["schema"] == "vc.agent-audience-binding.v1", binding
entry = binding["bindings"]["3"]
assert entry["audience"] == "james", entry
assert entry["provider_session_id"] == "codex-attach", entry
print(o["follower_pid"])
PY
)"
# The spawned follower actually delivers: a fresh addressed seal lands in the
# lease mailbox without any further command.
seal "James, dostawa przez attach." transcript_sealed 40
delivered=""
for _ in $(seq 1 50); do
  delivered="$(python3 - "$ATTACH_HOME" <<'PY'
import json
from pathlib import Path
import sys
leases = list((Path(sys.argv[1]) / "leases").glob("*.json"))
if leases:
    state = json.loads(leases[0].read_text(encoding="utf-8"))
    for item in state.get("pending", []):
        if item.get("kind") == "seal" and "attach" in (item.get("text") or ""):
            print("delivered")
PY
)"
  test -n "$delivered" && break
  sleep 0.1
done
test "$delivered" = "delivered"
receipt2="$(python3 "$DEMUX" --bus "$BUS" --bridge-home "$ATTACH_HOME" \
  --provider codex --session codex-attach --name james \
  --attach --channel 3)"
python3 - "$receipt2" "$follower_pid" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["follower_spawned"] is False, o
assert o["follower_pid"] == int(sys.argv[2]), o
PY
# A live lease heartbeat counts as a follower even without a pidfile, so a
# manually started follower is reused instead of spawning a doomed sibling.
rm -f "$ATTACH_HOME"/runtime/followers/*.pid
receipt3="$(python3 "$DEMUX" --bus "$BUS" --bridge-home "$ATTACH_HOME" \
  --provider codex --session codex-attach --name james \
  --attach --channel 3)"
python3 - "$receipt3" "$follower_pid" <<'PY'
import json, sys
o = json.loads(sys.argv[1])
assert o["follower_spawned"] is False, o
assert o["follower_pid"] == int(sys.argv[2]), o
PY
kill "$follower_pid" 2>/dev/null || true

# Channel claims preserve owners and concurrent writes. No live bus or audio.
python3 - "$DEMUX" "$WORKDIR/channel-claims" <<'PYTEST'
import importlib.util
import json
import multiprocessing
import subprocess
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("channel_claim_test", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
root = Path(sys.argv[2])
path = module.write_channel_binding(root, "2", "Roman", "codex", "owner")
original = path.read_bytes()
module.write_channel_binding(root, "2", "roman", "codex", "owner")
assert path.read_bytes() == original
for name, provider, session in [("roman", "codex", "other"), ("roman", "claude-code", "owner"), ("eve", "codex", "owner")]:
    result = subprocess.run([
        sys.executable, sys.argv[1], "--bridge-home", str(root),
        "--bus", str(root / "unused-bus"), "--attach", "--channel", "2",
        "--name", name, "--provider", provider, "--session", session,
    ], capture_output=True, text=True)
    assert result.returncode == 3, result
    assert "occupied by roman" in result.stderr, result.stderr
    assert "free channels: 1, 3, 4, 5, 6, 7, 8, 9" in result.stderr
    assert not result.stdout, result.stdout
    assert path.read_bytes() == original
    assert not (root / "runtime").exists(), "refusal must not start a follower"

ctx = multiprocessing.get_context("fork")
def race(folder, channels):
    barrier = ctx.Barrier(len(channels))
    def claim(slot, owner):
        barrier.wait(timeout=10)
        try:
            module.write_channel_binding(folder, slot, owner, "codex", owner)
        except OSError:
            sys.exit(3)
    workers = [ctx.Process(target=claim, args=(slot, f"owner-{i}")) for i, slot in enumerate(channels)]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join(timeout=15)
        assert not worker.is_alive(), "channel claim deadlocked"
    return [worker.exitcode for worker in workers]

shared = root / "same-slot"
assert sorted(race(shared, ["2"] * 8)) == [0] + [3] * 7
assert len(json.loads((shared / module.AUDIENCE_BINDING_FILENAME).read_text())["bindings"]) == 1
separate = root / "distinct-slots"
assert race(separate, [str(n) for n in range(1, 10)]) == [0] * 9
assert len(json.loads((separate / module.AUDIENCE_BINDING_FILENAME).read_text())["bindings"]) == 9
for invalid in ["{broken", "{}", '{"schema":"wrong","bindings":{}}']:
    path.write_text(invalid)
    try:
        module.write_channel_binding(root, "3", "new", "codex", "new")
    except OSError:
        pass
    else:
        raise AssertionError("corrupt bindings must refuse writes")
    assert path.read_text() == invalid
PYTEST

# --say resolves voice and speed from the profile store, not from hardcodes.
python3 - "$DEMUX" "$ATTACH_HOME" <<'PY'
import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("bus_demux_profile_test", sys.argv[1])
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
profile = module.voice_profile(Path(sys.argv[2]), "james")
assert profile["voice"] == "sal", profile
assert profile["speed"] == 1.1, profile
fallback = module.voice_profile(Path(sys.argv[2]), "nieznany")
assert fallback["voice"] == "leo", fallback
assert fallback["speed"] == module.DEFAULT_SPEECH_SPEED, fallback
PY

# --ack takes several delivery ids, all or nothing: one unknown id refuses the
# whole call and writes no marker; repeats and already-acknowledged ids are
# harmless; a single id keeps its old one-line receipt.
MULTI_HOME="$WORKDIR/multi-ack"
: >"$BUS"
seal "James, pierwsza." transcript_sealed 201
seal "James, druga." transcript_sealed 202
seal "James, trzecia." transcript_sealed 203
python3 "$DEMUX" --bus "$BUS" --bridge-home "$MULTI_HOME" \
  --provider codex --session codex-multi-ack --name james --from-start >/dev/null
python3 - "$DEMUX" "$BUS" "$MULTI_HOME" <<'PY'
import json, subprocess, sys
from pathlib import Path
demux, bus, home = sys.argv[1:]
state = json.loads(next((Path(home) / "leases").glob("*.json")).read_text())
ids = [item["delivery_id"] for item in state["pending"]]
assert len(ids) == 3, state["pending"]
markers = Path(home) / "acknowledgments" / state["lease_id"]
base = ["python3", demux, "--bus", bus, "--bridge-home", home,
        "--provider", "codex", "--session", "codex-multi-ack", "--ack"]
unknown = "f" * 24
for refused_ids in ([ids[0], unknown], [ids[0], "not-hex"]):
    result = subprocess.run(base + refused_ids, capture_output=True, text=True)
    assert result.returncode == 3, result
    assert "nothing acknowledged" in result.stderr, result.stderr
    assert not result.stdout, result.stdout
    assert not markers.exists() or not list(markers.iterdir()), list(markers.iterdir())
result = subprocess.run(base + [ids[0], ids[1], ids[0]], capture_output=True, text=True, check=True)
receipts = [json.loads(line) for line in result.stdout.splitlines()]
assert [row["delivery_id"] for row in receipts] == [ids[0], ids[1]], receipts
assert all(row["kind"] == "acknowledged" for row in receipts), receipts
assert {path.stem for path in markers.iterdir()} == {ids[0], ids[1]}
single = subprocess.run(base + [ids[2]], capture_output=True, text=True, check=True)
assert json.loads(single.stdout)["delivery_id"] == ids[2], single.stdout
again = subprocess.run(base + ids, capture_output=True, text=True, check=True)
assert len(again.stdout.splitlines()) == 3, again.stdout
status = json.loads(subprocess.run(
    ["python3", demux, "--bus", bus, "--bridge-home", home,
     "--provider", "codex", "--session", "codex-multi-ack", "--status"],
    capture_output=True, text=True, check=True).stdout)
assert status["backlog"] == 0, status
PY

# --attach --voice/--speed/--tts-vendor persist the name's profile into
# voices.json by a locked merge that keeps every other profile and key. The
# receipt names where the voice came from and whether the store exists.
VOICE_HOME="$WORKDIR/voice-attach"
mkdir -p "$VOICE_HOME"
python3 - "$VOICE_HOME" <<'PY'
import json, sys
from pathlib import Path
Path(sys.argv[1], "voices.json").write_text(json.dumps({
    "profiles": {"filip": {"voice": "rex", "speed": 1.25}},
    "note": "kept",
}))
PY
python3 - "$DEMUX" "$BUS" "$VOICE_HOME" "$WORKDIR/voice-fresh" <<'PY'
import json, os, signal, subprocess, sys
from pathlib import Path
demux, bus, home, fresh = sys.argv[1:]

def attach(root, channel, name, session, *extra):
    result = subprocess.run(
        ["python3", demux, "--bus", bus, "--bridge-home", root, "--attach",
         "--channel", channel, "--name", name, "--provider", "codex",
         "--session", session, *extra],
        capture_output=True, text=True)
    return result

followers = []
try:
    flagged = attach(home, "5", "James", "voice-session", "--voice", "sal",
                     "--speed", "1.3", "--tts-vendor", "openai")
    assert flagged.returncode == 0, flagged.stderr
    receipt = json.loads(flagged.stdout)
    followers.append(receipt["follower_pid"])
    assert receipt["voice_source"] == "flag", receipt
    assert receipt["voices_file"] == "present", receipt
    assert receipt["voice"]["voice"] == "sal" and receipt["voice"]["speed"] == 1.3, receipt
    store = json.loads(Path(home, "voices.json").read_text())
    assert store["note"] == "kept", store
    assert store["profiles"]["filip"] == {"voice": "rex", "speed": 1.25}, store
    assert store["profiles"]["james"] == {"voice": "sal", "speed": 1.3, "provider": "openai"}, store

    reused = attach(home, "5", "james", "voice-session")
    assert reused.returncode == 0, reused.stderr
    receipt = json.loads(reused.stdout)
    assert receipt["follower_spawned"] is False, receipt
    assert receipt["voice_source"] == "profile", receipt
    assert receipt["voice"]["voice"] == "sal", receipt

    unprofiled = attach(fresh, "6", "nowy", "fresh-session")
    assert unprofiled.returncode == 0, unprofiled.stderr
    receipt = json.loads(unprofiled.stdout)
    followers.append(receipt["follower_pid"])
    assert receipt["voice_source"] == "default", receipt
    assert receipt["voices_file"] == "missing", receipt
    assert receipt["voice"]["voice"] == "leo", receipt
finally:
    for pid in followers:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

# An unreadable profile store refuses before the channel is claimed.
broken = Path(fresh, "broken")
broken.mkdir()
(broken / "voices.json").write_text("{broken")
refused = attach(str(broken), "7", "zly", "broken-session", "--voice", "ara")
assert refused.returncode == 3, refused
assert "voice profiles are unreadable" in refused.stderr, refused.stderr
assert (broken / "voices.json").read_text() == "{broken"
assert not (broken / "vc.agent-audience-binding.v1.json").exists()
assert not (broken / "runtime").exists()
PY

# Concurrent profile writes never drop each other's names.
python3 - "$DEMUX" "$WORKDIR/voice-race" <<'PY'
import importlib.util, json, multiprocessing, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("voice_race", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
root = Path(sys.argv[2])
ctx = multiprocessing.get_context("fork")
barrier = ctx.Barrier(8)
def write(index):
    barrier.wait(timeout=10)
    module.write_voice_profile(root, f"agent{index}", voice="rex", speed=None, vendor=None)
workers = [ctx.Process(target=write, args=(i,)) for i in range(8)]
for worker in workers:
    worker.start()
for worker in workers:
    worker.join(timeout=15)
    assert worker.exitcode == 0, worker.exitcode
profiles = json.loads((root / "voices.json").read_text())["profiles"]
assert sorted(profiles) == [f"agent{i}" for i in range(8)], profiles
PY

# --watch: one compact line per notable envelope from the session's follower
# events. Drafts, attach receipts and stderr noise stay out; a replayed delivery
# prints once; another lease's envelope is not this session's; text is capped.
WATCH_HOME="$WORKDIR/watch-bridge"
python3 - "$DEMUX" "$WATCH_HOME" <<'PY'
import importlib.util, json, subprocess, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("watch_test", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
home = Path(sys.argv[2])
lease = module.lease_identifier("codex", "watch-session")
other = module.lease_identifier("codex", "someone-else")

def envelope(delivery, **fields):
    row = {"schema": module.EVENT_SCHEMA, "lease_id": lease, "delivery_id": delivery,
           "kind": "revised", "status": "utterance_revised",
           "state_change_allowed": False, "routing_match": "audience", "text": "draft"}
    row.update(fields)
    return json.dumps(row, ensure_ascii=False)

long_text = "James, " + "a" * 800
lines = [
    "bus-demux: bus=/tmp/x name=james follow=1 lease=" + lease,
    json.dumps({"schema": module.ATTACH_SCHEMA, "kind": "attach", "lease_id": lease}),
    envelope("a" * 24),
    envelope("b" * 24, kind="seal", status="transcript_sealed",
             state_change_allowed=True, text=long_text),
    envelope("c" * 24, kind="seal", status="transcript_sealed", coverage="refused",
             text="James, pokrycie odrzucone."),
    envelope("d" * 24, kind="routing_ambiguity", status="transcript_sealed",
             routing_candidates=["ramon", "roman"], text="Raman, sprawdź."),
    envelope("b" * 24, kind="seal", status="transcript_sealed",
             state_change_allowed=True, text=long_text),
    envelope("e" * 24, lease_id=other, kind="seal", status="transcript_sealed",
             state_change_allowed=True, text="James, cudzy."),
]
log = home / "runtime" / "followers" / f"{lease}.log"
log.parent.mkdir(parents=True)
log.write_text("\n".join(lines) + "\n{partial", encoding="utf-8")
base = ["python3", sys.argv[1], "--bridge-home", str(home), "--watch", "--full", "--once"]
out = subprocess.run(base + ["--provider", "codex", "--session", "watch-session"],
                     capture_output=True, text=True, check=True)
rows = [json.loads(line) for line in out.stdout.splitlines()]
assert [row["delivery_id"] for row in rows] == ["b" * 24, "c" * 24, "d" * 24], rows
assert set(rows[0]) == {"kind", "status", "coverage", "sca", "delivery_id", "text"}, rows[0]
assert rows[0]["sca"] is True and len(rows[0]["text"]) == 500, rows[0]
assert rows[1]["coverage"] == "refused" and rows[1]["sca"] is False, rows[1]
assert rows[2]["kind"] == "routing_ambiguity" and rows[2]["sca"] is False, rows[2]
unscoped = subprocess.run(base + ["--from-file", str(log)],
                          capture_output=True, text=True, check=True)
assert len(unscoped.stdout.splitlines()) == 4, unscoped.stdout
missing = subprocess.run(base + ["--provider", "codex", "--session", "nobody"],
                         capture_output=True, text=True)
assert missing.returncode == 1 and "watch source missing" in missing.stderr, missing
PY

# Following --watch is line-buffered: a seal appended to the log reaches the
# monitor at once, a draft appended next to it does not.
python3 - "$DEMUX" "$WORKDIR/watch-follow.log" <<'PY'
import json, select, subprocess, sys, time
demux, log = sys.argv[1:]
open(log, "w").close()
watcher = subprocess.Popen(
    ["python3", demux, "--watch", "--from-file", log],
    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
try:
    time.sleep(0.4)
    with open(log, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"schema": "codescribe.agent-bridge.event.v1",
                                 "kind": "revised", "status": "utterance_revised",
                                 "delivery_id": "1" * 24, "text": "draft"}) + "\n")
        handle.write(json.dumps({"schema": "codescribe.agent-bridge.event.v1",
                                 "kind": "seal", "status": "transcript_sealed",
                                 "state_change_allowed": True,
                                 "delivery_id": "2" * 24, "text": "James, już."}) + "\n")
    ready, _, _ = select.select([watcher.stdout], [], [], 5)
    assert ready, "watch did not flush a live line"
    line = json.loads(watcher.stdout.readline())
    assert line["delivery_id"] == "2" * 24 and "text" not in line and "notice" in line, line
finally:
    watcher.terminate()
    watcher.communicate(timeout=5)
PY

# --provider claude-code without --session reads the provider's own
# CLAUDE_CODE_SESSION_ID; an explicit --session wins; codex is never guessed.
python3 - "$DEMUX" "$WORKDIR/env-session" <<'PY'
import importlib.util, json, os, subprocess, sys
spec = importlib.util.spec_from_file_location("env_session", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
base = ["python3", sys.argv[1], "--bridge-home", sys.argv[2], "--status"]
env = dict(os.environ, CLAUDE_CODE_SESSION_ID="env-session-id")
status = json.loads(subprocess.run(base + ["--provider", "claude-code"], env=env,
                                   capture_output=True, text=True, check=True).stdout)
assert status["lease_id"] == module.lease_identifier("claude-code", "env-session-id"), status
explicit = json.loads(subprocess.run(
    base + ["--provider", "claude-code", "--session", "explicit-id"], env=env,
    capture_output=True, text=True, check=True).stdout)
assert explicit["lease_id"] == module.lease_identifier("claude-code", "explicit-id"), explicit
codex = subprocess.run(base + ["--provider", "codex"], env=env, capture_output=True, text=True)
assert codex.returncode == 2 and "CLAUDE_CODE_SESSION_ID" in codex.stderr, codex
bare = {key: value for key, value in env.items() if key != "CLAUDE_CODE_SESSION_ID"}
unset = subprocess.run(base + ["--provider", "claude-code"], env=bare,
                       capture_output=True, text=True)
assert unset.returncode == 2, unset
PY

# Spawned follower output is readable while full envelopes remain available
# for --watch and recovery. The fixture uses isolated bridge roots only.
python3 - "$DEMUX" "$WORKDIR/follower-human" <<'PY'
import importlib.util, json, os, re, signal, stat, subprocess, sys, time
from pathlib import Path

demux, base = sys.argv[1:]
spec = importlib.util.spec_from_file_location("follower_human_test", demux)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for drafts in (False, True):
    root = Path(base, "drafts" if drafts else "seals")
    root.mkdir(parents=True)
    bus = root / "bus.jsonl"
    rows = []
    for sequence, status, words in (
        (1, "utterance_revised", "James, projekt jeszcze trwa."),
        (2, "utterance_revised", "James, projekt prawie gotowy."),
        (3, "transcript_sealed", "James, tekst końcowy."),
    ):
        rows.append({"schema": module.CLEAN_SCHEMA, "sequence": sequence,
                     "session_id": "test-session", "utterance_id": f"utterance-{sequence}",
                     "emitted_at": "2026-09-30T15:42:39Z", "status": status,
                     "audience": "james", "text": words, "source": "test_fixture"})
    bus.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows))
    lease = module.lease_identifier("codex", "human-session")
    log, events = module.follower_paths(root, lease)
    log.parent.mkdir(parents=True)
    command = ["python3", demux, "--bus", str(bus), "--bridge-home", str(root),
               "--provider", "codex", "--session", "human-session", "--name", "james",
               "--from-start", "--coalesce", "--follower-events", str(events),
               "--follower-channel", "2"]
    if drafts:
        command.append("--drafts")
    with log.open("w", encoding="utf-8") as output:
        subprocess.run(command, stdout=output, stderr=subprocess.PIPE, text=True, check=True)
    lines = log.read_text().splitlines()
    assert len(lines) == 2 + int(drafts), lines  # attach + seal + optional draft
    assert sum(" seal " in line for line in lines) == 1, lines
    assert sum(" draft " in line for line in lines) == int(drafts), lines
    assert all("delivery_owner" not in line and "capture_epoch" not in line for line in lines)
    assert any(re.search(r"seal\s+2·james id=[0-9a-f]{24}.*tekst końcowy", line) for line in lines)
    envelopes = [json.loads(line) for line in events.read_text().splitlines()]
    assert len(envelopes) == 2 + 2 * int(drafts), envelopes
    assert envelopes[-1]["delivery_owner"]["rail"] == "native_bus_demux"
    assert stat.S_IMODE(events.stat().st_mode) == 0o600
    status = json.loads(subprocess.run(command[:2] + ["--bridge-home", str(root),
        "--provider", "codex", "--session", "human-session", "--status"],
        capture_output=True, text=True, check=True).stdout)
    assert status["follower_log"] == str(log) and status["follower_events"] == str(events)
    watch = ["python3", demux, "--bridge-home", str(root), "--provider", "codex",
             "--session", "human-session", "--watch", "--once"]
    compact = subprocess.run(watch + ["--full"], capture_output=True, text=True, check=True)
    compact_rows = [json.loads(line) for line in compact.stdout.splitlines()]
    assert len(compact_rows) == 1 and compact_rows[0]["text"] == "James, tekst końcowy."
    human = subprocess.run(watch + ["--human"], capture_output=True, text=True, check=True)
    assert len(human.stdout.splitlines()) == len(lines), human.stdout
    assert "tekst końcowy" in human.stdout
    assert ("projekt prawie gotowy" in human.stdout) == drafts
    assert "projekt jeszcze trwa" not in human.stdout
    # An old JSON follower log remains a valid explicit watch source.
    old = subprocess.run(watch + ["--full", "--from-file", str(events)], capture_output=True, text=True, check=True)
    assert old.stdout == compact.stdout

line = module.human_line({"kind": "seal", "audience": "james", "text": "a" * 300,
                          "delivery_id": "f" * 24, "emitted_at": "2026-09-30T15:42:39Z"}, "2")
assert len(json.loads(line.split(" id=", 1)[1].split(" ", 1)[1])) == 200, line
assert "\n" not in line and "2·james" in line

# Exercise the public attach path, including startup handoff to --watch.
root = Path(base, "attached")
root.mkdir()
bus = root / "bus.jsonl"
bus.write_text("")
attached = subprocess.run(["python3", demux, "--bus", str(bus), "--bridge-home", str(root),
                           "--attach", "--channel", "2", "--name", "james",
                           "--provider", "codex", "--session", "attached-session"],
                          capture_output=True, text=True, check=True)
receipt = json.loads(attached.stdout)
pid = receipt["follower_pid"]
try:
    log = Path(receipt["follower_log"])
    events = Path(receipt["follower_events"])
    assert log.exists() and events.exists(), receipt
    assert " attach 2·james " in log.read_text(), log.read_text()
    with bus.open("a") as output:
        output.write(json.dumps({"schema": module.CLEAN_SCHEMA, "sequence": 1,
            "session_id": "test-session", "utterance_id": "attached-seal",
            "emitted_at": "2026-09-30T15:46:01Z", "status": "transcript_sealed",
            "audience": "james", "text": "James, słyszę cię."}, ensure_ascii=False) + "\n")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and "słyszę cię" not in log.read_text():
        time.sleep(0.05)
    assert " seal " in log.read_text() and "słyszę cię" in log.read_text()
    assert any(json.loads(row).get("text") == "James, słyszę cię." for row in events.read_text().splitlines())
    watch = subprocess.run(["python3", demux, "--bridge-home", str(root), "--watch", "--full",
                            "--once", "--provider", "codex", "--session", "attached-session"],
                           capture_output=True, text=True, check=True)
    assert json.loads(watch.stdout)["text"] == "James, słyszę cię."
finally:
    os.kill(pid, signal.SIGTERM)
PY

echo "bus-demux: ok"
