"""Exercise target accounting against disposable registered Git worktrees."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


GUARD = Path(__file__).resolve().parents[1] / "verify-zero-target.sh"


class TargetAccountingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codescribe-target-guard-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.main = self.root / "main repo"
        self.fleet = self.root / "fleet"
        self.worker = self.fleet / "worker one"
        self.other = self.fleet / "other worker"
        self.tmp = self.root / "tmp"
        self.main.mkdir()
        self.tmp.mkdir()
        self.git("init", cwd=self.main)
        self.git("config", "core.hooksPath", "/dev/null")
        self.git("-c", "commit.gpgsign=false", "-c", "user.name=Fixture",
                 "-c", "user.email=fixtures@example.invalid", "commit", "--allow-empty", "-m", "fixture")
        for name, path in [("worker", self.worker), ("other", self.other)]:
            self.git("worktree", "add", "-b", name, str(path))
        self.shared = self.main / "target"
        self.shared.mkdir()
        (self.shared / "keep").write_bytes(b"shared build cache")
        self.env = dict(os.environ, VZT_WORKTREES_ROOT=str(self.fleet), VZT_TMP_ROOT=str(self.tmp),
                        VZT_THRESHOLD_MIB="0")

    def git(self, *args, cwd=None):
        subprocess.run(["git", "-c", "core.hooksPath=/dev/null", *args],
                       cwd=cwd or self.main, check=True, capture_output=True)

    def run_guard(self, mode, cwd=None, **env):
        return subprocess.run(["bash", str(GUARD), mode], cwd=cwd or self.worker,
                              env=dict(self.env, **env), capture_output=True, text=True)

    def test_main_and_shared_target_alias_are_excluded(self):
        result = self.run_guard("--check", cwd=self.main)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        (self.worker / "target").symlink_to(self.shared, target_is_directory=True)
        result = self.run_guard("--check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.shared / "keep").read_bytes(), b"shared build cache")

    def test_current_target_fails_size_policy_but_dry_run_preserves_it(self):
        target = self.worker / "target"
        target.mkdir()
        (target / "artifact").write_bytes(b"owned fixture")
        self.assertEqual(self.run_guard("--check").returncode, 1)
        self.assertEqual(self.run_guard("--dry-run").returncode, 0)
        self.assertEqual((target / "artifact").read_bytes(), b"owned fixture")

    def test_other_worker_and_temp_names_are_inventory_only(self):
        target = self.other / "target"
        target.mkdir()
        (target / "keep").write_bytes(b"worker may still be active")
        orphan = self.tmp / "cs-cut6-target-old"
        orphan.mkdir()
        (orphan / "keep").write_bytes(b"name does not establish ownership")
        result = self.run_guard("--check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ownership/activity unknown", result.stdout)
        self.assertTrue((target / "keep").exists())
        self.assertTrue((orphan / "keep").exists())

    def test_unknown_symlink_and_non_directory_are_errors(self):
        target = self.worker / "target"
        target.symlink_to(self.root / "unknown", target_is_directory=True)
        self.assertEqual(self.run_guard("--check").returncode, 2)
        target.unlink()
        target.write_bytes(b"not a directory")
        self.assertEqual(self.run_guard("--check").returncode, 2)

    def test_sweep_always_refuses_without_mutation(self):
        target = self.worker / "target"
        target.mkdir()
        (target / "keep").write_bytes(b"unproved terminal status")
        result = self.run_guard("--sweep")
        self.assertEqual(result.returncode, 2)
        self.assertIn("no verified cleanup admission", result.stderr)
        self.assertEqual((target / "keep").read_bytes(), b"unproved terminal status")
        self.assertTrue((self.shared / "keep").exists())


if __name__ == "__main__":
    unittest.main()
