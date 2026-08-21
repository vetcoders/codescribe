#!/usr/bin/env bash
# Hermetic Codescribe bus -> Codex App Server -> final answer + interrupt test.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'kill "${BRIDGE_PID:-}" 2>/dev/null || true; rm -rf "$WORKDIR"' EXIT
BUS="$WORKDIR/transcript-events.jsonl"
RPC_LOG="$WORKDIR/rpc.jsonl"
OUT="$WORKDIR/out.txt"
ERR="$WORKDIR/err.txt"
: >"$BUS"
: >"$RPC_LOG"

python3 - "$ROOT/scripts/codex-voice-bridge.py" <<'PY'
import importlib.util, pathlib, sys
path = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("codex_voice_bridge", path)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(module)
assert module.strip_address_prefix("Hej James, sprawdź branch", "james") == "sprawdź branch"
assert module.is_stop_only("Przerwij!")
spoken = module.speech_text("Odpowiedź [tutaj](https://example.com).\n```sh\nsecret\n```", 200)
assert spoken == "Odpowiedź tutaj.", spoken
PY

seal() {
  local session_id="$1"
  local status="$2"
  local text="$3"
  python3 - "$BUS" "$session_id" "$status" "$text" <<'PY'
import json, sys
path, session_id, status, text = sys.argv[1:]
event = {
    "schema": "codescribe.transcript.v1",
    "sequence": 1,
    "session_id": session_id,
    "mode": "dictation",
    "utterance_id": 1,
    "emitted_at": "2026-08-21T00:00:00Z",
    "status": status,
    "text": text,
}
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(event, ensure_ascii=False) + "\n")
PY
}

wait_for() {
  local pattern="$1"
  local file="$2"
  for _ in $(seq 1 100); do
    if rg -q "$pattern" "$file"; then
      return 0
    fi
    sleep 0.05
  done
  echo "timed out waiting for $pattern in $file" >&2
  return 1
}

MOCK_CODEX_LOG="$RPC_LOG" python3 "$ROOT/scripts/codex-voice-bridge.py" \
  --name james \
  --cwd "$ROOT" \
  --bus "$BUS" \
  --codex-bin "$ROOT/tests/fixtures/mock_codex_app_server.py" \
  --no-tts \
  --exit-after-turns 2 \
  >"$OUT" 2>"$ERR" &
BRIDGE_PID=$!

wait_for 'ready name=james' "$ERR"
seal "session-one" "transcript_sealed" "James, hold first command"
wait_for '"method":"turn/start"' "$RPC_LOG"

# A named live draft must interrupt the in-flight turn before its seal is sent.
seal "session-two" "utterance_draft" "James, second command"
wait_for '"method":"turn/interrupt"' "$RPC_LOG"
seal "session-two" "transcript_sealed" "James, second command"

wait "$BRIDGE_PID"
BRIDGE_PID=""

python3 - "$RPC_LOG" "$OUT" <<'PY'
import json, pathlib, sys
rpc = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text().splitlines()]
out = pathlib.Path(sys.argv[2]).read_text()
methods = [item.get("method") for item in rpc]
assert methods.count("turn/start") == 2, methods
assert methods.count("turn/interrupt") == 1, methods
turns = [item for item in rpc if item.get("method") == "turn/start"]
assert turns[0]["params"]["input"][0]["text"] == "hold first command"
assert turns[1]["params"]["input"][0]["text"] == "second command"
assert "MOCK_FINAL: second command" in out, out
assert all(item.get("params", {}).get("approvalPolicy") != "on-request" for item in rpc)
PY

echo "codex-voice-bridge: ok"

# Explicit resume of a thread reported as active must fail before Bus Demux starts.
ACTIVE_LOG="$WORKDIR/active-rpc.jsonl"
: >"$ACTIVE_LOG"
if MOCK_CODEX_LOG="$ACTIVE_LOG" MOCK_THREAD_ACTIVE=1 \
  python3 "$ROOT/scripts/codex-voice-bridge.py" \
    --name james \
    --cwd "$ROOT" \
    --bus "$BUS" \
    --thread-id "01999999-0000-7000-8000-000000000099" \
    --codex-bin "$ROOT/tests/fixtures/mock_codex_app_server.py" \
    --no-tts \
    >"$WORKDIR/active-out.txt" 2>"$WORKDIR/active-err.txt"; then
  echo "expected active-thread ownership refusal" >&2
  exit 1
fi
rg -q "refusing active thread" "$WORKDIR/active-err.txt"

echo "codex-voice-bridge active-thread refusal: ok"
