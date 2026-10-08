"""Signed publisher bytes and every unrelated payload have distinct boundaries."""

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "publisher_manifest", Path(__file__).resolve().parents[1] / "lib/refresh-agent-publisher-manifest.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PublisherManifestTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "bin").mkdir()
        self.publisher = self.root / "bin/codescribe"
        self.publisher.write_bytes(b"unsigned-publisher")
        self.publisher.chmod(0o755)
        helper = self.root / "bin/cs-bus"
        helper.write_bytes(b"ordinary-helper")
        entries = [{"path": str(path.relative_to(self.root)), "bytes": path.stat().st_size,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "mode": "0755"}
                   for path in (self.publisher, helper)]
        self.manifest = self.root / "manifest.json"
        self.manifest.write_text(json.dumps({"schema": "codescribe.agent-bridge.bundle.v1", "files": entries}))

    def test_signing_refresh_changes_only_publisher_digest(self):
        before = json.loads(self.manifest.read_text())
        self.publisher.write_bytes(b"signed-publisher")
        MODULE.refresh(self.root)
        after = json.loads(self.manifest.read_text())
        self.assertEqual(before["files"][1], after["files"][1])
        self.assertEqual(after["files"][0]["sha256"], hashlib.sha256(b"signed-publisher").hexdigest())

    def test_helper_tampering_is_refused_without_rewriting_manifest(self):
        before = self.manifest.read_bytes()
        (self.root / "bin/cs-bus").write_bytes(b"tampered-helper")
        with self.assertRaises(ValueError):
            MODULE.refresh(self.root)
        self.assertEqual(self.manifest.read_bytes(), before)

    def test_missing_publisher_is_refused(self):
        self.publisher.unlink()
        with self.assertRaises(ValueError):
            MODULE.refresh(self.root)

    def test_redirected_publisher_is_refused(self):
        self.publisher.unlink()
        self.publisher.symlink_to(self.root / "bin/cs-bus")
        with self.assertRaises(ValueError):
            MODULE.refresh(self.root)

    def test_duplicate_manifest_entry_is_refused(self):
        value = json.loads(self.manifest.read_text())
        value["files"].append(value["files"][0])
        self.manifest.write_text(json.dumps(value))
        with self.assertRaises(ValueError):
            MODULE.refresh(self.root)


if __name__ == "__main__":
    unittest.main()
