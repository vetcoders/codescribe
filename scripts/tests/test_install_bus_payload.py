"""Exercise install-bus staging with a test-owned installer sink, never real home.

The shell target and payload stager are production code. The compiler substitute
only captures the payload handed to the installer; real Swift compilation is
verified separately by make verify-install-bus.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
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
        for name in ("install-bus.sh", "build-app.sh", "bus-demux.py", "cs-bus", "cs-say"):
            shutil.copy2(REPO / "scripts" / name, self.repo / "scripts" / name)
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


if __name__ == "__main__":
    unittest.main()
