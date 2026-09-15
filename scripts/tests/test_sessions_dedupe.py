"""Hermetic tests for scripts/sessions-dedupe.py; never touches ~/.codescribe."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "sessions-dedupe.py"


def wav_bytes(tag: bytes, length: int = 4096) -> bytes:
    payload = (tag * (length // len(tag) + 1))[:length]
    return b"RIFF" + length.to_bytes(4, "little") + b"WAVE" + payload


def qt_bytes(length: int = 4096) -> bytes:
    header = b"\x00\x00\x00\x18ftypqt  \x00\x00\x00\x00qt  "
    return header + bytes(length - len(header))


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


class SessionsDedupeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.sessions = self.root / "sessions"
        self.sessions.mkdir()
        self.json_path = self.root / "report.json"

    def write(self, name: str, content: bytes, age: float = 3600.0) -> Path:
        path = self.sessions / name
        path.write_bytes(content)
        moment = time.time() - age
        os.utime(path, (moment, moment))
        return path

    def run_script(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--dir", str(self.sessions), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def report(self) -> dict:
        return json.loads(self.json_path.read_text())

    def test_dry_run_reports_group_and_changes_nothing(self):
        content = wav_bytes(b"alpha")
        paths = [
            self.write(name, content, age=age)
            for name, age in (("a.wav", 3600), ("b.wav", 3500), ("c.wav", 3400))
        ]
        other = self.write("d.wav", wav_bytes(b"omega"), age=3300)
        before = {
            path.name: (path.stat().st_ino, path.stat().st_nlink)
            for path in (*paths, other)
        }
        result = self.run_script("--json", str(self.json_path))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("dry-run", result.stdout)
        report = self.report()
        self.assertEqual(report["totals"]["duplicate_groups"], 1)
        group = report["groups"][0]
        self.assertEqual(group["canonical"], "a.wav")
        self.assertEqual(group["members"], ["a.wav", "b.wav", "c.wav"])
        self.assertEqual(
            {row["action"] for row in group["duplicates"]}, {"would-link"}
        )
        self.assertEqual(group["bytes_reclaimable"], 2 * len(content))
        self.assertEqual(report["totals"]["bytes_reclaimed"], 0)
        after = {
            path.name: (path.stat().st_ino, path.stat().st_nlink)
            for path in (*paths, other)
        }
        self.assertEqual(before, after)

    def test_apply_links_duplicates_and_preserves_bytes(self):
        content = wav_bytes(b"alpha")
        a = self.write("a.wav", content, age=3600)
        b = self.write("b.wav", content, age=3500)
        c = self.write("c.wav", content, age=3400)
        other_content = wav_bytes(b"omega")
        d = self.write("d.wav", other_content, age=3300)
        result = self.run_script("--apply", "--json", str(self.json_path))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len({p.stat().st_ino for p in (a, b, c)}), 1)
        for path in (a, b, c):
            self.assertEqual(path.stat().st_nlink, 3)
            self.assertEqual(sha256(path.read_bytes()), sha256(content))
        self.assertEqual(d.stat().st_nlink, 1)
        self.assertEqual(sha256(d.read_bytes()), sha256(other_content))
        report = self.report()
        self.assertEqual(
            {row["action"] for row in report["groups"][0]["duplicates"]}, {"linked"}
        )
        self.assertEqual(report["totals"]["bytes_reclaimed"], 2 * len(content))
        again = self.run_script("--json", str(self.json_path))
        self.assertEqual(again.returncode, 2, again.stderr)
        self.assertEqual(self.report()["totals"]["bytes_reclaimable"], 0)

    def test_shell_reported_and_renamed_only_with_flag(self):
        shell = self.write("ghost.wav", qt_bytes(), age=3600)
        digest_before = sha256(shell.read_bytes())

        refused = self.run_script("--rename-shells")
        self.assertEqual(refused.returncode, 2)
        self.assertIn("--rename-shells", refused.stderr)
        self.assertTrue(shell.exists())

        dry = self.run_script("--json", str(self.json_path))
        self.assertEqual(dry.returncode, 0, dry.stderr)
        shells = self.report()["shells"]
        self.assertEqual(
            [(row["name"], row["container"], row["action"]) for row in shells],
            [("ghost.wav", "mov", "reported")],
        )
        self.assertTrue(shell.exists())

        applied = self.run_script("--apply", "--json", str(self.json_path))
        self.assertEqual(applied.returncode, 0, applied.stderr)
        self.assertEqual(self.report()["shells"][0]["action"], "reported")
        self.assertTrue(shell.exists())
        self.assertEqual(sha256(shell.read_bytes()), digest_before)

        renamed = self.run_script(
            "--apply", "--rename-shells", "--json", str(self.json_path)
        )
        self.assertEqual(renamed.returncode, 0, renamed.stderr)
        self.assertIn("ghost.wav -> ghost.mov", renamed.stdout)
        self.assertFalse(shell.exists())
        moved = self.sessions / "ghost.mov"
        self.assertEqual(sha256(moved.read_bytes()), digest_before)
        row = self.report()["shells"][0]
        self.assertEqual((row["action"], row["renamed_to"]), ("renamed", "ghost.mov"))

    def test_last_session_wav_is_never_linked_or_renamed(self):
        content = wav_bytes(b"alpha")
        protected = self.write("last_session.wav", content, age=3600)
        session = self.write("s1.wav", content, age=3500)
        result = self.run_script(
            "--apply", "--rename-shells", "--json", str(self.json_path)
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertNotEqual(protected.stat().st_ino, session.stat().st_ino)
        self.assertEqual(protected.stat().st_nlink, 1)
        self.assertEqual(session.stat().st_nlink, 1)
        self.assertEqual(sha256(protected.read_bytes()), sha256(content))
        report = self.report()
        self.assertEqual(report["totals"]["duplicate_groups"], 0)
        self.assertIn(
            {"name": "last_session.wav", "reason": "protected"}, report["skipped"]
        )

    def test_recently_modified_file_refuses_without_force(self):
        self.write("fresh.wav", wav_bytes(b"alpha"), age=5)
        refused = self.run_script()
        self.assertEqual(refused.returncode, 3)
        self.assertIn("in flight", refused.stderr)
        self.assertFalse(self.json_path.exists())
        forced = self.run_script("--force", "--json", str(self.json_path))
        self.assertEqual(forced.returncode, 2, forced.stderr)
        self.assertEqual(self.report()["totals"]["duplicate_groups"], 0)

    def test_symlink_is_skipped_and_reported(self):
        real = self.write("real.wav", wav_bytes(b"alpha"), age=3600)
        link = self.sessions / "link.wav"
        os.symlink(real.name, link)
        result = self.run_script("--json", str(self.json_path))
        self.assertEqual(result.returncode, 2, result.stderr)
        report = self.report()
        self.assertEqual(report["totals"]["regular_files"], 1)
        self.assertEqual(report["totals"]["symlinks_skipped"], 1)
        self.assertIn({"name": "link.wav", "reason": "symlink"}, report["skipped"])
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), "real.wav")
        self.assertEqual(sha256(real.read_bytes()), sha256(wav_bytes(b"alpha")))


if __name__ == "__main__":
    unittest.main()
