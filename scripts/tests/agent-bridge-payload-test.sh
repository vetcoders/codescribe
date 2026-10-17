#!/usr/bin/env bash
# Hermetic staging/Creator lifecycle proof. Compiles only the Swift installer;
# no app build, real home-directory write, microphone or network access.
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
import subprocess
assert manifest["helper_version"] == "9.8.7", manifest
version_line = next(line.strip() for line in (root / "skills/codescribe/SKILL.md").read_text().splitlines()
                    if line.strip().startswith("version:"))
assert manifest["skill_version"] == version_line.split(":", 1)[1].strip().strip('"'), manifest
assert manifest["source_commit"] == subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()

source_skill = root / "skills" / "codescribe"
# Staging deliberately drops these. Naming them here, rather than mirroring the
# source wholesale, is the point: an expectation derived from the source tree
# DEMANDS whatever junk the tree happens to hold, which is how a Finder dropping
# came to ship with a checksum in a signed manifest.
EXCLUDED_NAMES = {".DS_Store"}
expected = {
    f"skills/codescribe/{path.relative_to(source_skill).as_posix()}"
    for path in source_skill.rglob("*")
    if path.is_file() and path.name not in EXCLUDED_NAMES
}
expected.add("bin/bus-demux.py")
expected.update({"bin/cs-bus", "bin/cs-say", "bin/cs-junie", "bin/bus-mcp.py",
                 "bin/bus-dot-mcp.py", "bin/bus-mcp-events.py", "bin/bus-mcp-runtime.py", "bin/bus-mcp-key-store.py"})
expected.update({
    "plugins/.claude-plugin/marketplace.json",
    "plugins/codescribe/.claude-plugin/plugin.json",
    "plugins/codescribe/.mcp.json",
    "plugins/codescribe/hooks/hooks.json",
    "plugins/codescribe/hooks/claude-code.json",
    "plugins/codescribe/hooks/session.py",
    "plugins/codescribe/hooks/pre/codescribe-omp.ts",
    "plugins/codescribe/skills/codescribe/SKILL.md",
})
listed = {entry["path"] for entry in manifest["files"]}
actual = {
    path.relative_to(payload).as_posix()
    for path in payload.rglob("*")
    if path.is_file() and path.name != "manifest.json"
}
assert listed == expected == actual, (listed ^ expected, actual ^ expected)

# Independent of every mirror above: no Finder dropping may reach the payload or
# earn a checksum in the manifest, whatever the source tree holds. Falsified by
# removing the ignore filter from stage_agent_bridge — it comes straight back.
assert not [p for p in listed if p.rsplit("/", 1)[-1] in EXCLUDED_NAMES], listed
assert not list(payload.rglob(".DS_Store")), "Finder dropping staged into payload"
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

# Destructive destinations are proven against a disposable fake checkout. If
# the guard regresses, only the temp tree can be replaced — never this repo.
FAKE_ROOT="$WORKDIR/unsafe-repo"
mkdir -p "$FAKE_ROOT/scripts" "$FAKE_ROOT/skills/codescribe"
cp "$ROOT/scripts/build-app.sh" "$FAKE_ROOT/scripts/build-app.sh"
cp "$ROOT/scripts/bus-demux.py" "$FAKE_ROOT/scripts/bus-demux.py"
cp "$ROOT/scripts/cs-bus" "$ROOT/scripts/cs-say" "$ROOT/scripts/cs-junie" "$ROOT/scripts/bus-mcp.py" "$FAKE_ROOT/scripts/"
cp "$ROOT/scripts/bus-dot-mcp.py" "$ROOT/scripts/bus-mcp-events.py" "$ROOT/scripts/bus-mcp-runtime.py" "$ROOT/scripts/bus-mcp-key-store.py" "$FAKE_ROOT/scripts/"
cp -R "$ROOT/plugins" "$FAKE_ROOT/plugins"
cp "$ROOT/skills/codescribe/SKILL.md" "$FAKE_ROOT/skills/codescribe/SKILL.md"
if "$FAKE_ROOT/scripts/build-app.sh" --stage-agent-bridge "$FAKE_ROOT" 9.8.7 \
  >"$WORKDIR/unsafe-root.out" 2>"$WORKDIR/unsafe-root.err"; then
  echo "repo-root staging destination was not refused" >&2
  exit 1
fi
grep -q "refusing unsafe agent bridge destination" "$WORKDIR/unsafe-root.err"
test -f "$FAKE_ROOT/skills/codescribe/SKILL.md"

ln -s "$FAKE_ROOT/scripts/bus-demux.py" "$FAKE_ROOT/skills/codescribe/linked-helper.py"
if "$FAKE_ROOT/scripts/build-app.sh" \
  --stage-agent-bridge "$WORKDIR/symlink-payload" 9.8.7 \
  >"$WORKDIR/symlink.out" 2>"$WORKDIR/symlink.err"; then
  echo "symlinked bridge source was not refused" >&2
  exit 1
fi
grep -q "source may not contain symlinks" "$WORKDIR/symlink.err"

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
cat >"$WORKDIR/install.swift" <<'SWIFT'
import Foundation
@main
struct Install {
  static func main() throws {
    let installer = RealAgentBridgeInstaller(
      resourceRoot: URL(fileURLWithPath: CommandLine.arguments[1]),
      homeDirectory: URL(fileURLWithPath: CommandLine.arguments[2]),
      environment: [:])
    if CommandLine.arguments.count > 3 && CommandLine.arguments[3] == "creator" {
      // The production Update skill button reaches this exact public method.
      _ = try installer.install(selectedClients: [.codex])
    } else {
      _ = try installer.installRuntime()
    }
  }
}
SWIFT
xcrun swiftc -swift-version 6 -warnings-as-errors \
  "$ROOT/macos/Codescribe/Services/AgentBridgeInstaller.swift" \
  "$WORKDIR/install.swift" -o "$WORKDIR/install"
"$WORKDIR/install" "$PAYLOAD" "$WORKDIR/home"
LOCAL_BIN="$WORKDIR/home/.local/bin"
test ! -L "$LOCAL_BIN/cs-bus"
test ! -L "$LOCAL_BIN/cs-say"
test -f "$LOCAL_BIN/bus-mcp.py"
test ! -x "$LOCAL_BIN/bus-mcp.py"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | "$LOCAL_BIN/cs-bus" --mcp >"$WORKDIR/native-mcp.json"
python3 - "$WORKDIR/native-mcp.json" <<'PY'
import json,sys
tools = json.load(open(sys.argv[1]))["result"]["tools"]
assert any(tool["name"] == "codescribe_wait" for tool in tools)
PY
test -f "$LOCAL_BIN/cs-junie"
test ! -L "$LOCAL_BIN/cs-junie"
"$LOCAL_BIN/cs-junie" --help | grep -q -- '--channel'
BEFORE_BUS_VERSION="$("$LOCAL_BIN/cs-bus" --version)"
BEFORE_SAY_VERSION="$("$LOCAL_BIN/cs-say" --version)"
# Reproduce the older app's explicit replacement with a valid old-style payload.
# It ships only bus-demux.py, replacing runtime/bin and the generation receipt.
# Direct commands must survive, report their own generation and remain updatable.
cp -R "$PAYLOAD" "$WORKDIR/old-payload"
python3 - "$WORKDIR/old-payload" <<'PY'
import hashlib,json,sys
from pathlib import Path
p = Path(sys.argv[1])
for name in ('cs-bus', 'cs-say'):
    (p/'bin'/name).unlink()
helper = p/'bin/bus-demux.py'
helper.write_text('#!/usr/bin/env python3\nprint("old app helper")\n')
m = json.loads((p/'manifest.json').read_text())
m['bundle_version'] = '1.2.3'
m.pop('helper_version', None)
m.pop('source_commit', None)
m['files'] = [f for f in m['files'] if f['path'] not in ('bin/cs-bus', 'bin/cs-say')]
for f in m['files']:
    if f['path'] == 'bin/bus-demux.py':
        f['bytes'] = helper.stat().st_size
        f['sha256'] = hashlib.sha256(helper.read_bytes()).hexdigest()
(p/'manifest.json').write_text(json.dumps(m))
PY
"$WORKDIR/install" "$WORKDIR/old-payload" "$WORKDIR/home"
test ! -e "$WORKDIR/home/.codescribe/agent-bridge/runtime/bin/cs-bus"
test ! -e "$WORKDIR/home/.codescribe/agent-bridge/runtime/bin/cs-say"
OUTPUT="$(cd "$WORKDIR" && "$LOCAL_BIN/cs-bus" --bus "$BUS" --name james --once)"
"$LOCAL_BIN/cs-say" --help | grep -q -- '--provider'
"$LOCAL_BIN/cs-bus" --version | grep -q '^cs-bus 9.8.7+g'
"$LOCAL_BIN/cs-say" --version | grep -q '^cs-say 9.8.7+g'
test "$("$LOCAL_BIN/cs-bus" --version)" = "$BEFORE_BUS_VERSION"
test "$("$LOCAL_BIN/cs-say" --version)" = "$BEFORE_SAY_VERSION"
"$WORKDIR/install" "$PAYLOAD" "$WORKDIR/home"
test "$("$LOCAL_BIN/cs-bus" --version)" = "$BEFORE_BUS_VERSION"
python3 - "$OUTPUT" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
assert value["kind"] == "seal", value
assert value["state_change_allowed"] is True, value
PY

python3 - "$WORKDIR/install" "$PAYLOAD" "$WORKDIR/creator-home" <<'PY'
import base64, hashlib, importlib.util, json, os, shutil, subprocess, sys
from pathlib import Path

installer, payload, home = map(lambda p: Path(p).resolve(), sys.argv[1:])
# Model the existing Creator installation's ordinary command directory. Do not
# ask the ownership guard to infer a not-yet-created HOME or redirected path.
(home / '.local/bin').mkdir(parents=True)
modules = ('bus-dot-mcp.py', 'bus-mcp-events.py', 'bus-mcp-runtime.py', 'bus-mcp-key-store.py')
bridge = home / '.codescribe/agent-bridge'
runtime = bridge / 'runtime'
session = '11111111-2222-4333-8444-555555555555'
meta = {'io.modelcontextprotocol/protocolVersion': '2026-07-28',
        'io.modelcontextprotocol/clientCapabilities': {}}

def update(source=payload):
    # Real production installer; no substitute that copies files itself.
    result = subprocess.run([str(installer), str(source), str(home), 'creator'],
                            capture_output=True, text=True, timeout=20)
    assert result.returncode == 0, f'Creator installer rc={result.returncode}: {result.stderr}'

def wire(script, frames, *flags):
    env = {**os.environ, 'PYTHONHOME': '/nonexistent-host-python',
           'PYTHONPATH': '/nonexistent-host-path'}
    result = subprocess.run([sys.executable, '-E', str(runtime / 'bin' / script),
        '--scope', str(scope), *flags], input=''.join(json.dumps(f) + '\n' for f in frames),
        cwd=home, env=env, capture_output=True, text=True, check=True, timeout=15)
    return [json.loads(line) for line in result.stdout.splitlines()], result.stderr

def frame(method, values, identity=1):
    return {'jsonrpc': '2.0', 'id': identity, 'method': method,
            'params': {'_meta': meta, **values}}

def tool(name, arguments, identity=1):
    return frame('tools/call', {'name': 'codescribe_' + name, 'arguments': arguments}, identity)

def verified_modules():
    manifest = json.loads((runtime / 'manifest.json').read_text())
    for name in modules:
        entry = next(e for e in manifest['files'] if e['path'] == 'bin/' + name)
        assert hashlib.sha256((runtime / entry['path']).read_bytes()).hexdigest() == entry['sha256']

update()
verified_modules()
spec = importlib.util.spec_from_file_location('creator_lifecycle_demux', runtime / 'bin/bus-demux.py')
demux = importlib.util.module_from_spec(spec); sys.modules[spec.name] = demux; spec.loader.exec_module(demux)
lease = demux.lease_identifier('codex', session)
owner = {'provider': 'codex', 'provider_session_id': session, 'lease_id': lease}
bus = bridge / 'buses/channel-2.jsonl'; bus.parent.mkdir(); bus.touch()
ids = [f'{n:024x}' for n in range(1, 6)]
pending = [{**owner, 'delivery_owner': dict(owner), 'delivery_id': identity, 'bus': str(bus),
    'kind': 'message', 'source': 'typed', 'text': 'Iwo', 'emitted_at': '2026-10-15T12:00:00Z',
    'channel': '2', 'origin_channel': '0', 'audience': '*'} for identity in ids]
lease_path = bridge / 'leases' / (lease + '.json')
demux.atomic_json(lease_path, {'schema': demux.LEASE_SCHEMA, **owner, 'name': 'Miksa',
    'bus': str(bus), 'pending': pending, 'cursor': 42, 'wakeup': 'off'})
scope = runtime / 'miksa-mcp' / lease / 'bus-scope.json'
demux.atomic_json(scope, {'principal': 'synthetic-creator-owner', 'provider': 'codex',
    'session_id': session, 'name': 'Miksa', 'bus': str(bus), 'bridge_home': str(bridge),
    'message_targets': 'current_bus', 'broadcast_enabled': True})
state_before = lease_path.read_bytes()
scope_before = scope.read_bytes()
scope_inode = scope.stat().st_ino
seen = []

for n, identity in enumerate(ids):
    if n == 1:
        # Reproduce the confirmed Update skill removal with a valid app payload
        # that lacks the MCP modules. The next canonical payload supplies them.
        old = home.parent / 'creator-payload-without-mcp'
        shutil.copytree(payload, old)
        manifest = json.loads((old / 'manifest.json').read_text())
        for name in modules:
            (old / 'bin' / name).unlink()
        manifest['files'] = [e for e in manifest['files'] if e['path'] not in {'bin/' + m for m in modules}]
        (old / 'manifest.json').write_text(json.dumps(manifest))
        ack = bridge / 'acknowledgments' / lease / (ids[0] + '.json')
        ack_before = ack.read_bytes()
        update(old)
        assert all(not (runtime / 'bin' / name).exists() for name in modules)
        assert ack.read_bytes() == ack_before
    # Repeat the exact Creator install path on every subsequent delivery.
    update()
    verified_modules()
    assert lease_path.read_bytes() == state_before
    assert scope.read_bytes() == scope_before and scope.stat().st_ino == scope_inode
    rows, _ = wire('bus-dot-mcp.py', [tool('read', {}), tool('ack', {'delivery_ids': [identity]}, 2)])
    assert all('result' in row for row in rows), rows
    receipt = rows[0]['result']['structuredContent']
    assert receipt['read_delivery_ids'] == [identity]
    assert receipt['deliveries'][0]['text'] == 'Iwo'
    assert receipt['deliveries'][0]['addressed_to'] == '0 (all audience)'
    seen.extend(receipt['read_delivery_ids'])
assert seen == ids
rows, _ = wire('bus-dot-mcp.py', [tool('read', {})])
assert rows[0]['result']['structuredContent']['read_delivery_ids'] == []
assert rows[0]['result']['structuredContent']['remaining'] == 0
assert len(list((bridge / 'acknowledgments' / lease).glob('*.json'))) == 5

secret = 'whsec_' + base64.b64encode(b'only-synthetic-creator-test-key00').decode()
delivery = {'mode': 'webhook', 'url': 'https://synthetic.example.test/PRIVATE?token=PRIVATE'}
rows, errors = wire('bus-mcp-runtime.py', [frame('events/list', {}),
    frame('events/subscribe', {'name': 'codescribe.mailbox.ready', 'arguments': {},
        'delivery': {**delivery, 'secret': secret}, 'cursor': None}, 2),
    frame('events/unsubscribe', {'name': 'codescribe.mailbox.ready', 'arguments': {}, 'delivery': delivery}, 3),
    frame('events/unsubscribe', {'name': 'codescribe.mailbox.ready', 'arguments': {}, 'delivery': delivery}, 4)],
    '--callback-preflight')
assert rows[0]['result']['events'][0]['name'] == 'codescribe.mailbox.ready'
assert rows[1]['error']['data'] == {'reason': 'host_unpinned', 'callbackHost': 'synthetic.example.test'}
assert [row.get('result') for row in rows[2:]] == [{}, {}]
assert secret not in json.dumps(rows) + errors and 'PRIVATE' not in json.dumps(rows) + errors
assert lease_path.read_bytes() == state_before
assert not (scope.parent / 'events.sqlite').exists()
print('creator-payload-lifecycle: five distinct read/ACK deliveries survive repeated Update skill; preflight inert')
PY

echo "agent-bridge-payload: ok"
