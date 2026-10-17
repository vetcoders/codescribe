"""Version preparation against disposable Git histories; never touches the live repo."""

import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "release-version.py"
SPEC = importlib.util.spec_from_file_location("release_version", SCRIPT)
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseVersionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "fixture"\nversion = "0.1.0"\n\n[dependencies]\nexample = { version = "0.1.0" }\n')
        (self.root / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "fixture"\nversion = "0.1.0"\n\n[[package]]\nname = "example"\nversion = "0.1.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n')
        (self.root / "README.md").write_text('badge/version-0.1.0-blue\nThe current source version is `0.1.0`.\nFounder draft stays.\n')
        (self.root / "CHANGELOG.md").write_text('# Changelog\n\n## Unreleased\n\n- Human-authored feature explanation.\n\n## [0.1.0] - 2026-10-01\n\nInitial release.\n')
        self.commit("initial")
        self.git("tag", "v0.1.0")
        (self.root / "feature.txt").write_text("new source\n")
        self.commit("feat: Show recording status\n\n* fix: Keep original PCM\n\n* fix: Keep original PCM\n\nAuthored-By: Agent <private@example.invalid>\nprovider_session_id: private-session\n")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args], text=True).strip()

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "-q", "-m", message)

    def args(self, command="bump", **extra):
        values = dict(root=str(self.root), version_file="Cargo.toml", command=command,
                      kind="patch", to_version=None, version="0.1.1", since=None,
                      until="HEAD", date="2026-10-08", inventory_only=False)
        values.update(extra)
        return argparse.Namespace(**values)

    def run_release(self, **extra):
        with contextlib.redirect_stdout(io.StringIO()):
            release.run(self.args(**extra))

    def snapshot(self):
        return {str(p.relative_to(self.root)): p.read_bytes() for p in self.root.rglob("*")
                if p.is_file() and ".git" not in p.relative_to(self.root).parts}

    def test_bump_promotes_notes_and_preserves_other_versions_and_drafts(self):
        self.run_release()
        cargo = (self.root / "Cargo.toml").read_text()
        self.assertEqual(release.cargo_version(cargo), "0.1.1")
        self.assertIn('example = { version = "0.1.0" }', cargo)
        locked = (self.root / "Cargo.lock").read_text()
        self.assertIn('name = "fixture"\nversion = "0.1.1"', locked)
        self.assertIn('name = "example"\nversion = "0.1.0"', locked)
        self.assertIn("Founder draft stays.", (self.root / "README.md").read_text())
        self.assertIn("badge/version-0.1.1-blue", (self.root / "README.md").read_text())
        changelog = (self.root / "CHANGELOG.md").read_text()
        self.assertIn("## Unreleased\n\nNo changes recorded yet.", changelog)
        self.assertIn("Human-authored feature explanation.", changelog)
        self.assertIn("Keep original PCM", changelog)
        receipt = json.loads((self.root / "docs/releases/0.1.1-commits.json").read_text())
        self.assertEqual(len(receipt["commits"]), 1)
        self.assertEqual(receipt["commits"][0]["squash_headings"], ["fix: Keep original PCM"])
        self.assertNotIn("private-session", json.dumps(receipt))
        self.assertNotIn("private@example", json.dumps(receipt))

    def test_repeated_pending_bump_and_explicit_target_are_byte_identical(self):
        self.run_release()
        before = self.snapshot()
        self.run_release()
        self.assertEqual(before, self.snapshot())
        self.run_release(to_version="0.1.1")
        self.assertEqual(before, self.snapshot())

    def test_tag_closes_pending_version_and_next_bump_excludes_previous_history(self):
        self.run_release()
        self.commit("chore: Prepare version")
        self.git("tag", "v0.1.1")
        (self.root / "feature.txt").write_text("next source\n")
        self.commit("fix: Release microphone promptly")
        self.run_release()
        receipt = json.loads((self.root / "docs/releases/0.1.2-commits.json").read_text())
        self.assertEqual([c["subject"] for c in receipt["commits"]], ["fix: Release microphone promptly"])

    def test_existing_human_summary_is_preserved_on_refresh(self):
        text = (self.root / "CHANGELOG.md").read_text()
        text += "\n## [0.1.1] - 2026-10-08\n\nA concise human description.\n"
        (self.root / "CHANGELOG.md").write_text(text)
        self.run_release(command="notes")
        first = self.snapshot()
        self.run_release(command="notes")
        self.assertEqual(first, self.snapshot())
        self.assertIn("A concise human description.", (self.root / "CHANGELOG.md").read_text())
        self.assertNotIn("### Fixed", (self.root / "CHANGELOG.md").read_text())

    def test_invalid_boundary_and_disagreeing_manifest_leave_files_untouched(self):
        before = self.snapshot()
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_release(since="missing-release")
        self.assertEqual(before, self.snapshot())
        cargo = self.root / "Cargo.toml"
        cargo.write_text(cargo.read_text().replace('name = "fixture"\nversion = "0.1.0"', 'name = "fixture"\nversion = "0.2.0"'))
        before = self.snapshot()
        with self.assertRaisesRegex(ValueError, "versions disagree"):
            self.run_release()
        self.assertEqual(before, self.snapshot())

    def test_write_failure_rolls_back_all_files_and_new_inventory(self):
        before = self.snapshot()
        original_write = release.atomic_write
        failed = False
        def fail_once(path, data, mode=0o644):
            nonlocal failed
            if path.name == "Cargo.toml" and not failed:
                failed = True
                raise OSError("injected disk write failure")
            original_write(path, data, mode)
        with patch.object(release, "atomic_write", side_effect=fail_once):
            with self.assertRaisesRegex(OSError, "injected"):
                self.run_release()
        self.assertEqual(before, self.snapshot())
        self.assertFalse((self.root / ".git/codescribe-version-transaction.json").exists())

    def test_interrupted_transaction_recovers_and_refuses_foreign_edit(self):
        one, two = self.root / "one", self.root / "two"
        one.write_bytes(b"after")
        two.write_bytes(b"before")
        journal = self.root / ".git/codescribe-version-transaction.json"
        entries = [{"path": str(p), "before": release.encode(b"before"),
                    "after": release.encode(b"after"), "mode": 0o644} for p in (one, two)]
        journal.write_text(json.dumps({"files": entries}))
        two.write_bytes(b"foreign")
        with self.assertRaisesRegex(ValueError, "newer edit"):
            release.recover(journal)
        self.assertEqual(one.read_bytes(), b"after")
        self.assertTrue(journal.exists())
        two.write_bytes(b"before")
        release.recover(journal)
        self.assertEqual(one.read_bytes(), b"before")
        self.assertEqual(two.read_bytes(), b"before")
        self.assertFalse(journal.exists())

    def test_concurrent_preparation_edit_is_never_overwritten(self):
        path = self.root / "README.md"
        before = path.read_bytes()
        path.write_bytes(b"newer Founder edit")
        with self.assertRaisesRegex(ValueError, "Concurrent edit"):
            release.transact({path: b"prepared version"}, self.root / ".git/journal", {path: before})
        self.assertEqual(path.read_bytes(), b"newer Founder edit")

    def test_locked_metadata_accepts_all_bumped_workspace_owners(self):
        # Root integrator runs this metadata-only Cargo invocation, never a worker.
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = [".", "core"]\nresolver = "2"\n\n[workspace.package]\nversion = "0.1.0"\n\n[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2021"\n')
        (self.root / "core/src").mkdir(parents=True)
        (self.root / "core/Cargo.toml").write_text('[package]\nname = "fixture-core"\nversion.workspace = true\nedition = "2021"\n')
        (self.root / "core/src/lib.rs").write_text("")
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("")
        (self.root / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "fixture"\nversion = "0.1.0"\n\n[[package]]\nname = "fixture-core"\nversion = "0.1.0"\n')
        self.run_release()
        metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--offline",
            "--no-deps", "--format-version", "1", "--manifest-path", str(self.root / "Cargo.toml")], text=True))
        self.assertEqual({p["name"]: p["version"] for p in metadata["packages"]},
                         {"fixture": "0.1.1", "fixture-core": "0.1.1"})
        self.assertFalse((self.root / "target").exists())


if __name__ == "__main__":
    unittest.main()
