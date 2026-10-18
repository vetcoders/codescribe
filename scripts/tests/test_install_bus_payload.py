"""Exercise install-bus staging with a test-owned installer sink, never real home.

The shell target and payload stager are production code. The compiler substitute
only captures the payload handed to the installer; real Swift compilation is
verified separately by make verify-install-bus.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class InstallBusPayloadTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.work = Path(temporary.name)
        self.repo = self.work / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        (self.repo / "skills" / "codescribe").mkdir(parents=True)
        for name in ("install-bus.sh", "build-app.sh", "bus-demux.py", "cs-bus", "cs-say", "cs-junie", "bus-mcp.py",
                     "bus-mcp-agent.py", "bus-mcp-events.py", "bus-mcp-runtime.py", "bus-mcp-key-store.py"):
            shutil.copy2(REPO / "scripts" / name, self.repo / "scripts" / name)
        shutil.copytree(REPO / "plugins", self.repo / "plugins")
        shutil.copy2(REPO / "Cargo.toml", self.repo / "Cargo.toml")
        shutil.copy2(REPO / "skills/codescribe/SKILL.md", self.repo / "skills/codescribe/SKILL.md")
        self.runtime = self.work / "bridge" / "runtime"
        (self.runtime / "bin").mkdir(parents=True)
        self.publisher = self.runtime / "bin" / "codescribe"
        self.publisher.write_bytes(b"#!/bin/sh\nexit 97\n")
        self.publisher.chmod(0o755)
        self.manifest = {"schema": "codescribe.agent-bridge.bundle.v1", "files": [{
            "path": "bin/codescribe", "bytes": self.publisher.stat().st_size, "mode": "0755",
            "sha256": hashlib.sha256(self.publisher.read_bytes()).hexdigest()}]}
        (self.runtime / "manifest.json").write_text(json.dumps(self.manifest))
        self.receipt = self.work / "received.json"
        compiler = self.work / "tools" / "xcrun"
        compiler.parent.mkdir()
        compiler.write_text('''#!/usr/bin/env python3
import pathlib, sys
output = pathlib.Path(sys.argv[sys.argv.index("-o") + 1])
output.write_text(''' + repr('''#!/usr/bin/env python3
import hashlib, json, os, pathlib, sys
payload = pathlib.Path(sys.argv[1])
manifest = json.loads((payload / "manifest.json").read_text())
entry = next(item for item in manifest["files"] if item["path"] == "bin/codescribe")
publisher = payload / "bin/codescribe"
assert publisher.is_file() and not publisher.is_symlink()
assert publisher.stat().st_size == entry["bytes"]
assert hashlib.sha256(publisher.read_bytes()).hexdigest() == entry["sha256"]
assert os.access(publisher, os.X_OK)
pathlib.Path(os.environ["TEST_INSTALL_RECEIPT"]).write_text(json.dumps(entry))
''') + ''' )
output.chmod(0o755)
''')
        compiler.chmod(0o755)
        self.env = {**os.environ, "PATH": str(compiler.parent) + os.pathsep + os.environ["PATH"],
                    "CODESCRIBE_AGENT_BRIDGE_HOME": str(self.runtime.parent),
                    "TEST_INSTALL_RECEIPT": str(self.receipt)}

    def install(self):
        return subprocess.run([str(self.repo / "scripts/install-bus.sh")], env=self.env,
                              text=True, capture_output=True, timeout=30)

    def test_helper_install_carries_the_verified_existing_publisher(self):
        original = self.publisher.read_bytes()
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.receipt.read_text()), self.manifest["files"][0])
        self.assertEqual(self.publisher.read_bytes(), original)

    def test_changed_publisher_refuses_before_installation(self):
        self.publisher.write_bytes(b"changed artifact")
        before = self.publisher.read_bytes()
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("publisher", result.stderr)
        self.assertFalse(self.receipt.exists())
        self.assertEqual(self.publisher.read_bytes(), before)

    def test_missing_managed_publisher_does_not_select_an_ambient_command(self):
        self.publisher.unlink()
        (self.runtime / "manifest.json").unlink()
        ambient = self.work / "tools" / "codescribe"
        ambient.write_text("#!/bin/sh\nexit 0\n")
        ambient.chmod(0o755)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("publisher", result.stderr)
        self.assertFalse(self.receipt.exists())
        self.assertFalse(self.publisher.exists())

    def test_symlink_publisher_refuses_before_installation(self):
        target = self.work / "foreign-publisher"
        self.publisher.rename(target)
        self.publisher.symlink_to(target)
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("publisher", result.stderr)
        self.assertFalse(self.receipt.exists())
        self.assertTrue(self.publisher.is_symlink())

    def test_missing_manifest_preserves_the_publisher(self):
        (self.runtime / "manifest.json").unlink()
        before = self.publisher.read_bytes()
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("publisher", result.stderr)
        self.assertFalse(self.receipt.exists())
        self.assertEqual(self.publisher.read_bytes(), before)

    def test_managed_update_keeps_exact_read_ack_and_adopts_current_bus_helpers(self):
        # Exercise shipped entrypoints outside the checkout across a payload
        # replacement. The second generation adds an informational field;
        # mailbox state stays outside the replaceable payload.
        payload = self.work / 'payload'
        spec = importlib.util.spec_from_file_location('upgrade_contract_demux', self.repo / 'scripts/bus-demux.py')
        demux = importlib.util.module_from_spec(spec); sys.modules[spec.name] = demux; spec.loader.exec_module(demux)
        bridge = (self.work / 'mailbox').resolve(); bus = (self.work / 'events.jsonl').resolve(); bus.touch()
        session = '11111111-2222-4333-8444-555555555555'
        lease = demux.lease_identifier('codex', session)
        owner = {'provider': 'codex', 'provider_session_id': session, 'lease_id': lease}
        ids = [f'{n:024x}' for n in range(1, 6)]
        pending = [{**owner, 'delivery_owner': dict(owner), 'delivery_id': identity,
            'bus': str(bus), 'kind': 'message', 'source': 'typed', 'text': 'hello',
            'channel': '2', 'origin_channel': '0', 'audience': '*', 'emitted_at': '2026-10-15T12:00:00Z'}
            for identity in ids]
        demux.atomic_json(bridge / 'leases' / (lease + '.json'),
            {'schema': demux.LEASE_SCHEMA, **owner, 'name': 'Ada', 'bus': str(bus), 'pending': pending,
             'cursor': 42, 'wakeup': 'off'})
        scope = self.work / 'scope.json'
        scope.write_text(json.dumps({'principal': 'test-owner', 'provider': 'codex', 'session_id': session,
            'name': 'Ada', 'bus': str(bus), 'bridge_home': str(bridge),
            'state_dir': str(bridge / 'runtime' / 'agent-mcp'), 'message_targets': 'current_bus'}))
        meta = {'io.modelcontextprotocol/protocolVersion': '2026-07-28',
                'io.modelcontextprotocol/clientCapabilities': {}}

        def stage():
            result = subprocess.run([str(self.repo / 'scripts/build-app.sh'), '--stage-agent-bridge',
                str(payload), '9.8.7'], text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((payload / 'manifest.json').read_text())
            for name in ('bus-mcp-agent.py', 'bus-mcp-runtime.py', 'bus-mcp-events.py', 'bus-mcp-key-store.py'):
                self.assertTrue(any(e['path'] == 'bin/' + name for e in manifest['files']))

        def calls(names):
            requests = [{'jsonrpc': '2.0', 'id': n, 'method': 'tools/call',
                'params': {'_meta': meta, 'name': 'codescribe_' + name, 'arguments': arguments}}
                for n, (name, arguments) in enumerate(names, 1)]
            env = {**os.environ, 'PYTHONHOME': '/nonexistent-host-python', 'PYTHONPATH': '/nonexistent-host-path'}
            result = subprocess.run([sys.executable, '-E', str(payload / 'bin/bus-mcp-agent.py'),
                '--scope', str(scope)], input=''.join(json.dumps(r) + '\n' for r in requests),
                cwd=self.work, env=env, text=True, capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = [json.loads(line) for line in result.stdout.splitlines()]
            self.assertTrue(all('result' in r for r in rows), rows)
            return [r['result']['structuredContent'] for r in rows]

        stage()
        first = calls([('read', {}), ('ack', {'delivery_ids': [ids[0]]})])
        self.assertEqual(first[0]['read_delivery_ids'], ids[:1])
        self.assertEqual(first[0]['deliveries'][0]['addressed_to'], '0 (all audience)')
        helper = self.repo / 'scripts/bus-mcp.py'
        old = helper.read_text()
        changed = old.replace('    def status(self):\n        result = self.run(["--status"])',
            '    def status(self):\n        result = self.run(["--status"])\n        result["future_additive_field"] = True')
        self.assertNotEqual(changed, old); helper.write_text(changed)
        stage()
        self.assertIn('future_additive_field', (payload / 'bin/bus-mcp.py').read_text())
        rest = [('status', {})]
        for identity in ids[1:]:
            rest += [('read', {}), ('ack', {'delivery_ids': [identity]})]
        rest.append(('read', {}))
        second = calls(rest)
        reads = [r for r in second if r.get('read_delivery_ids')]
        self.assertEqual([r['read_delivery_ids'][0] for r in reads], ids[1:])
        self.assertTrue(all(r['deliveries'][0]['text'] == 'hello' for r in reads))
        self.assertEqual(second[-1]['remaining'], 0)
        self.assertEqual(second[-1]['read_delivery_ids'], [])


if __name__ == "__main__":
    unittest.main()
