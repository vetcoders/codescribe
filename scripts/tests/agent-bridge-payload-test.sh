#!/usr/bin/env bash
# Hermetic staging proof for the signed-app agent bridge payload. No Xcode, app,
# home-directory write, microphone, or network access.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
PAYLOAD="$WORKDIR/Codescribe.app/Contents/Resources/agent-bridge"

"$ROOT/scripts/build-app.sh" --stage-agent-bridge "$PAYLOAD" 9.8.7
FIRST_MANIFEST="$(shasum -a 256 "$PAYLOAD/manifest.json" | awk '{print $1}')"

python3 - "$ROOT" "$PAYLOAD" <<'PY'
import hashlib
import json
import os
import sys
from pathlib import Path

root = Path(sys.argv[1])
payload = Path(sys.argv[2])
manifest = json.loads((payload / "manifest.json").read_text(encoding="utf-8"))
assert manifest["schema"] == "codescribe.agent-bridge.bundle.v1", manifest
assert manifest["bundle_version"] == "9.8.7", manifest
assert manifest["helper"] == "bin/bus-demux.py", manifest
assert manifest["skill"] == "skills/codescribe", manifest

source_skill = root / "skills" / "codescribe"
expected = {
    f"skills/codescribe/{path.relative_to(source_skill).as_posix()}"
    for path in source_skill.rglob("*")
    if path.is_file()
}
expected.add("bin/bus-demux.py")
listed = {entry["path"] for entry in manifest["files"]}
actual = {
    path.relative_to(payload).as_posix()
    for path in payload.rglob("*")
    if path.is_file() and path.name != "manifest.json"
}
assert listed == expected == actual, (listed ^ expected, actual ^ expected)
for entry in manifest["files"]:
    path = payload / entry["path"]
    assert hashlib.sha256(path.read_bytes()).hexdigest() == entry["sha256"], entry
    assert path.stat().st_size == entry["bytes"], entry
assert os.access(payload / "bin" / "bus-demux.py", os.X_OK)
PY

# Re-stage over a tampered payload. The atomic update restores canonical bytes
# and produces an identical deterministic manifest.
printf 'tampered\n' >>"$PAYLOAD/bin/bus-demux.py"
"$ROOT/scripts/build-app.sh" --stage-agent-bridge "$PAYLOAD" 9.8.7
SECOND_MANIFEST="$(shasum -a 256 "$PAYLOAD/manifest.json" | awk '{print $1}')"
test "$FIRST_MANIFEST" = "$SECOND_MANIFEST"

# The packaged helper runs after leaving the checkout: its runtime path has no
# dependency on repository-relative imports or files.
BUS="$WORKDIR/transcript-events.jsonl"
python3 - "$BUS" <<'PY'
import json, sys
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    handle.write(json.dumps({
        "schema": "codescribe.transcript.v1",
        "sequence": 1,
        "session_id": "payload-test",
        "status": "transcript_sealed",
        "text": "James, payload działa.",
    }, ensure_ascii=False) + "\n")
PY
OUTPUT="$(cd "$WORKDIR" && python3 "$PAYLOAD/bin/bus-demux.py" --bus "$BUS" --name james --once)"
python3 - "$OUTPUT" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
assert value["kind"] == "seal", value
assert value["state_change_allowed"] is True, value
PY

echo "agent-bridge-payload: ok"
