from contextlib import redirect_stderr, redirect_stdout
import fcntl
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import io
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch


loader = SourceFileLoader("restart_if_idle", str(Path(__file__).parents[1] / "restart-if-idle"))
spec = spec_from_loader(loader.name, loader)
restart = module_from_spec(spec)
loader.exec_module(restart)


class RestartIfIdleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        # main() resolves --app; /var/folders resolves under /private.
        self.root = Path(self.temporary.name).resolve()
        self.app = self.root / "Codescribe.app"
        self.handshakes = self.root / "restart-if-idle"
        self.handshakes.mkdir()
        self.main = self.app / "Contents/MacOS/Codescribe"
        self.ffi = self.app / "Contents/Frameworks/libcodescribe_ffi.dylib"
        for path in (self.main, self.ffi):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture installed payload")
        with (self.app / "Contents/Info.plist").open("wb") as handle:
            plistlib.dump({"CFBundleVersion": "fixture", "CSBuildCommit": "expected"}, handle)
        self.running = {123}
        self.calls = []
        self.outcome = "accepted"
        self.outcomes = []
        self.loaded_is_current = True
        self.stale_pids = set()
        self.lsof_fails = False
        self.spoken = []
        self.publish_ready(123, "old-generation")

    def publish_ready(self, pid, nonce):
        (self.handshakes / f"{pid}.ready.json").write_text(json.dumps({
            "schema": "codescribe.restart-if-idle.ready.v1", "pid": pid,
            "nonce": nonce, "bundle": str(self.app), "commit": "expected",
        }))

    def run_command(self, arguments):
        self.calls.append(arguments)
        executable = Path(arguments[0]).name
        output = ""
        if executable == "pgrep":
            output = "\n".join(str(pid) for pid in sorted(self.running))
        elif executable == "cs-bus":
            output = str(self.root / "agent-turn.lock")
        elif executable == "afplay":
            pass
        elif executable == "say":
            self.spoken.append(arguments[-1])
        elif executable == "codesign":
            pass
        elif executable == "open" and "-a" in arguments:
            url = arguments[-1]
            request_id = url.split("request=", 1)[1]
            request = json.loads((self.handshakes / f"{request_id}.request.json").read_text())
            if self.outcomes:
                self.outcome = self.outcomes.pop(0)
            receipt = {
                "schema": "codescribe.restart-if-idle.receipt.v1", "request": request_id,
                "pid": 999 if self.outcome == "wrong-owner" else request["pid"],
                "nonce": request["nonce"],
                "status": "refused" if self.outcome == "busy" else "accepted",
                "reason": "capture_busy_or_not_ready" if self.outcome == "busy" else None,
            }
            (self.handshakes / f"{request_id}.receipt.json").write_text(json.dumps(receipt))
            if self.outcome == "accepted":
                self.running.clear()
                (self.handshakes / f"{request['pid']}.ready.json").unlink()
        elif executable == "open":
            self.running = {456}
            self.publish_ready(456, "new-generation")
        elif executable == "lsof":
            if self.lsof_fails:
                return subprocess.CompletedProcess(arguments, 1, stdout="", stderr="")
            pid = int(arguments[arguments.index("-p") + 1])
            current = self.loaded_is_current and pid not in self.stale_pids
            output = f"p{pid}\n"
            for path in (self.main, self.ffi):
                inode = path.stat().st_ino if current else 0
                output += f"ftxt\ni{inode}\nn{path}\n"
        else:
            self.fail(f"Unexpected external action: {arguments}")
        return subprocess.CompletedProcess(arguments, 0, stdout=output, stderr="")

    def invoke(self, quit_only=False):
        with patch.object(restart, "run", self.run_command):
            return restart.restart(self.app, self.handshakes, 0.01, quit_only)

    def test_old_process_without_native_protocol_gets_no_quit_or_open(self):
        (self.handshakes / "123.ready.json").unlink()
        with self.assertRaisesRegex(restart.Refusal, "does_not_support_atomic_restart"):
            self.invoke()
        self.assertEqual([Path(call[0]).name for call in self.calls], ["pgrep"])
        self.assertEqual(self.running, {123})

    def test_busy_native_owner_is_left_running_without_relaunch(self):
        self.outcome = "busy"
        with self.assertRaisesRegex(restart.Refusal, "capture_busy_or_not_ready"):
            self.invoke()
        opens = [call for call in self.calls if Path(call[0]).name == "open"]
        self.assertEqual(len(opens), 1)
        self.assertIn("codescribe://restart-if-idle", opens[0][-1])
        self.assertEqual(self.running, {123})

    def test_mismatched_native_receipt_cannot_authorize_restart(self):
        self.outcome = "wrong-owner"
        with self.assertRaisesRegex(restart.Refusal, "identity_mismatch"):
            self.invoke()
        self.assertEqual(self.running, {123})

    def test_success_requires_new_process_and_current_loaded_payload(self):
        result = self.invoke()
        self.assertEqual(result["status"], "restarted")
        self.assertEqual((result["old_pid"], result["new_pid"]), (123, 456))
        self.assertTrue(result["loaded_matches_installed"])
        self.assertTrue(result["signature_verified"])
        self.assertEqual(result["commit"], "expected")
        self.assertFalse(any("osascript" in str(call) or "kill" in str(call) for call in self.calls))

    def test_stale_loaded_payload_is_not_a_successful_restart(self):
        self.loaded_is_current = False
        with self.assertRaisesRegex(restart.Refusal, "new_generation_not_verified"):
            self.invoke()

    def test_installer_quit_only_never_launches_another_generation(self):
        result = self.invoke(quit_only=True)
        self.assertEqual(result["status"], "quit_idle")
        self.assertFalse(self.running)
        self.assertEqual(len([call for call in self.calls if Path(call[0]).name == "open"]), 1)

    def test_exported_codescribe_gates_ride_the_relaunch_process(self):
        with patch.dict(os.environ, {"CODESCRIBE_APPLE_DICTATION_TRANSCRIBER": "1"}):
            result = self.invoke()
        self.assertEqual(result["status"], "restarted")
        self.assertEqual(
            result["carried_environment"], ["CODESCRIBE_APPLE_DICTATION_TRANSCRIBER"]
        )
        opens = [call for call in self.calls if Path(call[0]).name == "open"]
        self.assertEqual(len(opens), 2)
        self.assertIn("codescribe://restart-if-idle", opens[0][-1])
        relaunch = opens[1]
        self.assertIn("--env", relaunch)
        self.assertIn("CODESCRIBE_APPLE_DICTATION_TRANSCRIBER=1", relaunch)
        self.assertEqual(relaunch[-1], str(self.app))

    def invoke_main(self, *flags):
        stdout, stderr = io.StringIO(), io.StringIO()
        argv = ["restart-if-idle", "--app", str(self.app), "--timeout", "0.01", *flags]
        with patch.object(restart, "run", self.run_command), \
                patch.object(restart.time, "sleep", lambda seconds: None), \
                patch.object(restart.sys, "argv", argv), \
                redirect_stdout(stdout), redirect_stderr(stderr):
            code = restart.main()
        lines = [json.loads(line) for line in stdout.getvalue().splitlines()]
        waits = [json.loads(line) for line in stderr.getvalue().splitlines()]
        return code, lines[-1], waits

    def dispatches(self):
        return [call for call in self.calls if Path(call[0]).name == "open" and "-a" in call]

    def test_newer_only_leaves_a_current_process_alone(self):
        with patch.object(restart, "run", self.run_command):
            result = restart.restart(self.app, self.handshakes, 0.01, False, newer_only=True)
        self.assertEqual(result, {"status": "current", "pid": 123, "commit": "expected"})
        self.assertEqual([Path(call[0]).name for call in self.calls], ["pgrep", "lsof"])
        self.assertEqual(self.running, {123})

    def test_newer_only_refuses_when_the_loaded_identity_is_unreadable(self):
        self.lsof_fails = True
        with patch.object(restart, "run", self.run_command):
            with self.assertRaisesRegex(restart.Refusal, "loaded_identity_unavailable"):
                restart.restart(self.app, self.handshakes, 0.01, False, newer_only=True)
        self.assertFalse(self.dispatches())

    def test_newer_only_restarts_a_process_that_kept_the_replaced_binaries(self):
        self.stale_pids = {123}
        with patch.object(restart, "run", self.run_command):
            result = restart.restart(self.app, self.handshakes, 0.01, False, newer_only=True)
        self.assertEqual(result["status"], "restarted")
        self.assertEqual((result["old_pid"], result["new_pid"], result["old_commit"]), (123, 456, "expected"))

    def test_wait_retries_a_busy_owner_until_it_is_idle(self):
        self.stale_pids = {123}
        self.outcomes = ["busy", "busy", "accepted"]
        code, result, waits = self.invoke_main("--newer-only", "--wait", "600", "--interval", "1")
        self.assertEqual((code, result["status"]), (0, "restarted"))
        self.assertEqual([wait["reason"] for wait in waits], ["capture_busy_or_not_ready"] * 2)
        self.assertEqual(len(self.dispatches()), 3)

    def test_wait_ends_on_a_refusal_that_idling_cannot_change(self):
        self.stale_pids = {123}
        self.outcome = "wrong-owner"
        code, result, waits = self.invoke_main("--newer-only", "--wait", "600", "--interval", "1")
        self.assertEqual((code, result["reason"]), (75, "restart_receipt_identity_mismatch"))
        self.assertEqual((waits, len(self.dispatches())), ([], 1))

    def test_wait_never_retries_after_the_owner_accepted(self):
        self.stale_pids = {123}
        self.loaded_is_current = False
        code, result, waits = self.invoke_main("--wait", "600", "--interval", "1")
        self.assertEqual((code, result["reason"]), (75, "new_generation_not_verified"))
        self.assertEqual((waits, len(self.dispatches())), ([], 1))

    def test_a_second_waiter_defers_to_the_armed_one(self):
        held = os.open(self.handshakes / "wait.lock", os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, held)
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        code, result, _ = self.invoke_main("--newer-only", "--wait", "600")
        self.assertEqual((code, result["status"]), (0, "deferred_to_active_waiter"))
        self.assertEqual([Path(call[0]).name for call in self.calls], ["cs-bus"])

    def test_unattended_run_without_an_application_has_nothing_to_do(self):
        self.running = set()
        code, result, _ = self.invoke_main("--newer-only", "--announce")
        self.assertEqual((code, result["status"]), (0, "not_running"))
        self.assertFalse(self.spoken)

    def test_announce_says_done_after_a_verified_restart(self):
        self.stale_pids = {123}
        code, result, _ = self.invoke_main("--newer-only", "--announce")
        self.assertEqual((code, result["status"]), (0, "restarted"))
        self.assertEqual(self.spoken, ["Zrobione"])

    def test_announce_stays_silent_when_nothing_newer_is_installed(self):
        code, result, _ = self.invoke_main("--newer-only", "--announce")
        self.assertEqual((code, result["status"]), (0, "current"))
        self.assertFalse(self.spoken)


if __name__ == "__main__":
    unittest.main()
