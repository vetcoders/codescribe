"""Native queue acceptance is separate from conversation acknowledgment."""
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bus_native_queue_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class NativeQueueTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.session = "native-test-thread"
        self.lease_id = DEMUX.lease_identifier("codex", self.session)
        self.ids = [f"{index:024x}" for index in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id,
            "provider": "codex", "provider_session_id": self.session,
            "pending": self.pending,
        })

    def envelope(self, identity):
        return {
            "kind": "seal", "delivery_id": identity, "lease_id": self.lease_id,
            "provider": "codex", "provider_session_id": self.session,
            "delivery_owner": {"lease_id": self.lease_id, "provider": "codex",
                               "provider_session_id": self.session},
            "audience": "lena", "text": "Lena, pięć Iwo; `echo $HOME` nie jest kodem.",
            "state_change_allowed": False, "coverage": "refused",
            "wav": "/private/audio.wav", "sample_start": 123, "sample_end": 456,
        }

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, "queued", ""))
    def test_typed_message_uses_native_queue_without_audio_seal(self, run, _which):
        payload = dict(self.pending[0], kind="message", source="typed", state_change_allowed=True)
        for key in ("coverage", "wav", "sample_start", "sample_end"): payload.pop(key, None)
        self.pending[0] = payload
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        state["pending"] = self.pending
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", state)
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(self.result(payload["delivery_id"])["disposition"], "provider_accepted")
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 1)

    def wake(self):
        return DEMUX.NativeQueueWakeup(self.root, self.session, "2")

    def result(self, identity):
        return DEMUX.read_json(self.root / "wakeups" / self.lease_id / f"{identity}.json")

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, '{"queued":true}', ""))
    def test_five_occurrences_ordered_once_compact_and_not_acknowledged(self, run, _which):
        wake = self.wake()
        for payload in self.pending:
            wake.enqueue(payload)
            wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 5)
        for call, identity in zip(run.call_args_list, self.ids):
            argv = call.args[0]
            self.assertEqual(argv[:5], ["/fake/codex", "queue", "--thread", self.session, "--message"])
            message = argv[5]
            self.assertIn(self.pending[0]["text"], message)
            self.assertIn(identity, message)
            self.assertIn('"state_change_allowed": false', message)
            self.assertNotIn("sample_start", message)
            self.assertNotIn("/private/audio.wav", message)
            self.assertNotIn("shell", call.kwargs)
            self.assertEqual(self.result(identity)["disposition"], "provider_accepted")
            self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, identity))
        resumed = self.wake()
        for payload in self.pending:
            resumed.enqueue(payload)
        resumed.close(wait=True)
        self.assertEqual(run.call_count, 5)
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        self.assertEqual(state["pending"], self.pending)

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", side_effect=subprocess.TimeoutExpired("codex", 30))
    def test_timeout_is_uncertain_and_restart_does_not_replay(self, run, _which):
        for _ in range(2):
            wake = self.wake()
            wake.enqueue(self.pending[0])
            wake.close(wait=True)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(self.result(self.ids[0])["disposition"], "uncertain")

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 1, "", "server draining"))
    def test_rejection_retains_delivery_and_explicit_retry(self, run, _which):
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        self.assertEqual(self.result(self.ids[0])["disposition"], "rejected")
        run.return_value = subprocess.CompletedProcess([], 0, "queued", "")
        wake = self.wake()
        wake.enqueue(self.pending[0], retry=True)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(self.result(self.ids[0])["disposition"], "provider_accepted")
        self.assertEqual(self.result(self.ids[0])["attempt"], 2)

    @patch("subprocess.run")
    def test_foreign_owner_draft_missing_and_acknowledged_never_queue(self, run):
        invalid = [dict(self.pending[0], kind="draft"),
                   dict(self.pending[0], provider_session_id="other"),
                   dict(self.pending[0], delivery_owner={"provider_session_id": "other"}),
                   self.envelope("f" * 24)]
        DEMUX.atomic_json(self.root / "acknowledgments" / self.lease_id / f"{self.ids[1]}.json", {
            "lease_id": self.lease_id, "delivery_id": self.ids[1],
        })
        wake = self.wake()
        for payload in invalid + [self.pending[1]]:
            wake.enqueue(payload)
        wake.close(wait=True)
        run.assert_not_called()

    def test_attached_follower_wakes_without_blocking_bus_and_restores_same_cursor(self):
        bindir = self.root / "bin"
        bindir.mkdir()
        record = self.root / "provider-input.jsonl"
        executable = bindir / "codex"
        executable.write_text(
            f"#!{sys.executable}\nimport json,sys,time\n"
            f"with open({str(record)!r}, 'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n"
            "time.sleep(0.3)\nprint('queued')\n"
        )
        executable.chmod(0o755)
        bus = self.root / "channel.jsonl"
        bus.touch()
        environment = dict(os.environ, PATH=str(bindir) + os.pathsep + os.environ["PATH"])
        command = [sys.executable, str(SPEC.origin), "--bus", str(bus),
                   "--bridge-home", str(self.root / "attached"), "--attach",
                   "--channel", "2", "--name", "lena", "--provider", "codex",
                   "--session", "test-attached-thread"]
        receipt = json.loads(subprocess.check_output(command, env=environment, text=True))
        pid = receipt["follower_pid"]
        try:
            self.assertEqual(receipt["wakeup"], "codex-queue")
            with bus.open("a") as output:
                for index in range(5):
                    output.write(json.dumps({
                        "schema": DEMUX.CLEAN_SCHEMA, "session_id": f"take-{index}",
                        "utterance_id": f"utterance-{index}", "sequence": index,
                        "status": "transcript_sealed", "audience": "lena",
                        "text": "Lena, Iwo.",
                    }) + "\n")
            lease_file = self.root / "attached" / "leases" / f"{receipt['lease_id']}.json"
            deadline = time.monotonic() + 7
            while time.monotonic() < deadline:
                state = DEMUX.read_json(lease_file) or {}
                if len(state.get("pending", [])) == 5:
                    break
                time.sleep(0.02)
            self.assertEqual(len(state.get("pending", [])), 5)
            self.assertEqual(state["cursor"], bus.stat().st_size)
            while time.monotonic() < deadline:
                rows = record.read_text().splitlines() if record.exists() else []
                receipts = list((self.root / "attached" / "wakeups" / receipt["lease_id"]).glob("*.json"))
                if len(receipts) == 5 and all(DEMUX.read_json(p).get("disposition") == "provider_accepted" for p in receipts):
                    break
                time.sleep(0.02)
            self.assertEqual(len(rows), 5)
            self.assertEqual([json.loads(row)[2] for row in rows], ["test-attached-thread"] * 5)
            # Changing to monitor-only updates the actual follower, preserving unread occurrences.
            updated = json.loads(subprocess.check_output(command + ["--wakeup", "off"], env=environment, text=True))
            pid = updated["follower_pid"]
            self.assertEqual(updated["wakeup"], "off")
            self.assertNotEqual(pid, receipt["follower_pid"])
            self.assertEqual(updated["cursor"], state["cursor"])
            self.assertEqual(len(DEMUX.read_json(lease_file)["pending"]), 5)
            again = json.loads(subprocess.check_output(command + ["--wakeup", "off"], env=environment, text=True))
            self.assertFalse(again["follower_spawned"])
            self.assertEqual(again["follower_pid"], pid)
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass

    def test_busy_turn_bell_is_short_and_read_returns_full_original_without_ack(self):
        event = dict(self.pending[0], schema=DEMUX.EVENT_SCHEMA, status="transcript_sealed")
        events = self.root / "notifications.jsonl"
        events.write_text(json.dumps(event) + "\n")
        bell = subprocess.check_output([
            sys.executable, SPEC.origin, "--watch", "--once", "--from-file", str(events),
        ], text=True)
        self.assertEqual(json.loads(bell)["delivery_id"], event["delivery_id"])
        self.assertNotIn(event["text"], bell)
        self.assertLess(len(bell), 200)
        full = subprocess.check_output([
            sys.executable, SPEC.origin, "--watch", "--full", "--once", "--from-file", str(events),
        ], text=True)
        self.assertEqual(json.loads(full)["text"], event["text"])
        received = subprocess.check_output([
            sys.executable, SPEC.origin, "--read-delivery", event["delivery_id"],
            "--provider", "codex", "--session", self.session, "--bridge-home", str(self.root),
        ], text=True)
        self.assertEqual(json.loads(received), self.pending[0])
        self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, event["delivery_id"]))

    def test_public_help_names_installed_command_and_default_bell(self):
        help_text = subprocess.check_output([sys.executable, SPEC.origin, "--help"], text=True)
        self.assertTrue(help_text.startswith("usage: cs-bus"), help_text)
        self.assertNotIn("bus-demux.py", help_text)
        self.assertIn("default", help_text)
        self.assertIn("--full", help_text)


if __name__ == "__main__":
    unittest.main()
