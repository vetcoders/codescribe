from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
import json
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
        self.root = Path(self.temporary.name)
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
        self.loaded_is_current = True
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
        elif executable == "codesign":
            pass
        elif executable == "open" and "-a" in arguments:
            url = arguments[-1]
            request_id = url.split("request=", 1)[1]
            request = json.loads((self.handshakes / f"{request_id}.request.json").read_text())
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
        elif executable == "open":
            self.running = {456}
            self.publish_ready(456, "new-generation")
        elif executable == "lsof":
            output = "p456\n"
            for path in (self.main, self.ffi):
                inode = path.stat().st_ino if self.loaded_is_current else 0
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
        with self.assertRaisesRegex(restart.Refusal, "timeout_without_forced_termination"):
            self.invoke()

    def test_installer_quit_only_never_launches_another_generation(self):
        result = self.invoke(quit_only=True)
        self.assertEqual(result["status"], "quit_idle")
        self.assertFalse(self.running)
        self.assertEqual(len([call for call in self.calls if Path(call[0]).name == "open"]), 1)


if __name__ == "__main__":
    unittest.main()
