"""Run the installer's actual Python body against isolated bundles and leases."""

import contextlib
import fcntl
import io
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts/install-if-idle.sh"


class InstallPrebuiltTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.fixture = Path(temporary.name).resolve()
        self.source = self.fixture / "Public Release.app"
        self.destination = self.fixture / "Applications" / "Codescribe.app"
        self.lease = self.fixture / "agent-turn.lock"
        self.interlock = self.fixture / "runtime.lock"
        self.lease.touch()
        self.interlock.touch()
        self.source.joinpath("Contents").mkdir(parents=True)
        self.info = {
            "CFBundleIdentifier": "com.vetcoders.codescribe",
            "CFBundleShortVersionString": "0.15.2",
            "CFBundleVersion": "1835",
            "CSBuildCommit": "123456789",
        }
        self.write_info()
        self.source.joinpath("Contents", "payload").write_bytes(b"signed payload")
        body = SCRIPT.read_text().split("<<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
        definitions = body.split("\ntry:\n    from_app =", 1)[0]
        self.namespace = {}
        with patch.object(sys, "argv", [str(SCRIPT), str(REPO)]):
            exec(compile(definitions, str(SCRIPT), "exec"), self.namespace)
        self.namespace["DEST"] = self.destination
        self.namespace["cargo_version"] = lambda: "0.15.2"
        self.commands = []
        self.pids = []
        self.bus_checks = 0
        self.bus_busy_at = None
        self.quit_exits = True
        self.copy_fails = False
        self.signature_valid = True
        self.team = "MW223P3NPX"
        self.git_dirty = False
        self.output = io.StringIO()
        mocked = patch("subprocess.run", side_effect=self.run_command)
        mocked.start()
        self.addCleanup(mocked.stop)

    def write_info(self):
        with self.source.joinpath("Contents", "Info.plist").open("wb") as handle:
            plistlib.dump(self.info, handle)

    def run_command(self, argv, **kwargs):
        self.commands.append(list(argv))
        code, stdout, stderr = 0, "", ""
        if len(argv) > 2 and str(argv[1]).endswith("bus-demux.py"):
            flag = argv[2]
            if flag == "--assert-install-idle":
                self.bus_checks += 1
                code = 2 if self.bus_checks == self.bus_busy_at else 0
            elif flag == "--print-install-interlock-path":
                stdout = str(self.interlock)
            else:
                raise AssertionError(argv)
        elif argv[0] == "git":
            if "--is-inside-work-tree" in argv:
                stdout = "true\n"
            elif "status" in argv:
                stdout = " M app/lib.rs\n" if self.git_dirty else ""
            else:
                stdout = "123456789\n"
        elif argv[0] == "codesign":
            if "--verify" in argv:
                code = 0 if self.signature_valid else 1
            elif "--display" in argv:
                stderr = (
                    f"TeamIdentifier={self.team}\n"
                    f"Authority=Developer ID Application: Maciej Gad ({self.team})\n"
                )
            else:
                raise AssertionError("installation must not resign: " + repr(argv))
        elif argv[0] == "xcrun":
            self.assertEqual(argv[1:3], ["stapler", "validate"])
        elif argv[0] == "spctl":
            stderr = "accepted\nsource=Notarized Developer ID\n"
        elif argv[0] == "pgrep":
            code = 0 if self.pids else 1
            stdout = "\n".join(self.pids)
        elif argv[0] == "osascript":
            self.assertEqual(argv[1:], ["-e", 'quit app "Codescribe"'])
            if self.quit_exits:
                self.pids = []
        elif argv[0] == "rsync":
            if argv[1] == "--help":
                stdout = "--xattrs"
            elif self.copy_fails:
                code = 7
            else:
                with self.interlock.open("a") as contender:
                    with self.assertRaises(BlockingIOError):
                        fcntl.flock(contender.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.assertEqual(argv[-2:], [f"{self.source}/", f"{self.destination}/"])
                shutil.copytree(self.source, self.destination, dirs_exist_ok=True)
        elif argv[0] in ("lsof", "ps"):
            stdout = "fixture holder"
        else:
            raise AssertionError("unexpected product execution: " + repr(argv))
        result = subprocess.CompletedProcess(argv, code, stdout, stderr)
        if kwargs.get("check") and code:
            raise subprocess.CalledProcessError(code, argv, stdout, stderr)
        return result

    def install(self):
        with contextlib.redirect_stdout(self.output), contextlib.redirect_stderr(self.output):
            self.namespace["validate_bundle"](self.source, allow_destination=False)
            self.namespace["install_prebuilt"](self.source, self.lease)

    def assert_not_copied(self):
        self.assertFalse(self.destination.exists())
        self.assertFalse(any(c[0] == "rsync" and c[1] != "--help" for c in self.commands))

    def hold(self, path):
        handle = path.open("a")
        fcntl.flock(handle.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
        self.addCleanup(handle.close)
        return handle

    def test_valid_bundle_quits_idle_old_process_and_copies_without_build_or_resign(self):
        self.pids = ["4242"]
        with self.assertRaises(SystemExit) as terminal:
            self.install()
        self.assertEqual(terminal.exception.code, 0)
        self.assertEqual(self.destination.joinpath("Contents", "payload").read_bytes(), b"signed payload")
        quit_index = next(i for i, c in enumerate(self.commands) if c[0] == "osascript")
        copy_index = next(i for i, c in enumerate(self.commands) if c[:2] == ["rsync", "-a"])
        self.assertLess(quit_index, copy_index)
        self.assertFalse(self.pids)
        self.assertGreaterEqual(self.bus_checks, 4)
        self.assertTrue(any(c[0] == "codesign" and str(self.destination) in c for c in self.commands[copy_index + 1:]))

    def test_capture_starting_before_quit_refuses_without_quitting_or_copying(self):
        self.pids = ["4242"]
        self.bus_busy_at = 2
        with self.assertRaises(SystemExit) as terminal:
            self.install()
        self.assertEqual(terminal.exception.code, 2)
        self.assert_not_copied()
        self.assertFalse(any(c[0] == "osascript" for c in self.commands))

    def test_busy_agent_turn_refuses_before_quit_and_copy(self):
        self.pids = ["4242"]
        self.hold(self.lease)
        with self.assertRaises(SystemExit) as terminal:
            self.install()
        self.assertEqual(terminal.exception.code, 2)
        self.assert_not_copied()
        self.assertFalse(any(c[0] == "osascript" for c in self.commands))

    def test_failed_quit_leaves_the_old_bundle_untouched(self):
        self.pids = ["4242"]
        self.quit_exits = False
        with patch("time.monotonic", side_effect=[0, 31]), self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.assert_not_copied()

    def test_unavailable_runtime_interlock_refuses_copy(self):
        self.hold(self.interlock)
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.assert_not_copied()

    def test_capture_change_inside_the_install_interlock_refuses_copy(self):
        self.bus_busy_at = 3
        with self.assertRaises(SystemExit) as terminal:
            self.install()
        self.assertEqual(terminal.exception.code, 2)
        self.assert_not_copied()

    def test_wrong_commit_or_dirty_tree_refuses_before_any_idle_probe(self):
        self.info["CSBuildCommit"] = "987654321"
        self.write_info()
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.info["CSBuildCommit"] = "123456789"
        self.write_info()
        self.git_dirty = True
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.assertEqual(self.bus_checks, 0)
        self.assert_not_copied()

    def test_invalid_signature_or_foreign_team_refuses_before_any_idle_probe(self):
        self.signature_valid = False
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.signature_valid = True
        self.team = "OTHERTEAM"
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        self.assertEqual(self.bus_checks, 0)
        self.assert_not_copied()

    def test_failed_copy_releases_runtime_interlock(self):
        self.copy_fails = True
        with self.assertRaises(self.namespace["Refuse"]):
            self.install()
        with self.interlock.open("a") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


if __name__ == "__main__":
    unittest.main()
