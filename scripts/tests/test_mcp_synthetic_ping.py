"""Synthetic-only boundaries and cleanup; no production data or network."""
import importlib.util
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import MagicMock, patch

SOURCE = Path(__file__).resolve().parents[1] / "mcp-synthetic-ping.py"
SPEC = importlib.util.spec_from_file_location("synthetic_ping_test", SOURCE)
PING = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PING)


class SyntheticPingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.session, self.identity, self.bus = PING.fixture(self.root)
        self.mailbox = PING.PROBE.Mailbox("fixture", self.session, self.root, self.bus, self.root)

    def test_one_synthetic_message_and_no_production_owner(self):
        result = self.mailbox.call("read_pending", {})
        self.assertEqual(len(result["events"]), 1)
        delivery = result["events"][0]["delivery"]
        self.assertEqual(delivery["text"], "Miksa, ping")
        self.assertEqual(delivery["provider"], "fixture")
        self.assertEqual(delivery["bus"], str(self.bus))
        self.assertNotIn("channel", delivery)
        self.assertFalse((self.root / "wakeups").exists())

    def test_ack_never_calls_native_queue(self):
        self.mailbox.call("read_pending", {})
        with patch.object(PING.D, "withdraw_acknowledged_queue", side_effect=AssertionError("queue forbidden")):
            receipt = self.mailbox.call("ack", {"request_id": self.identity})
        self.assertFalse(receipt["native_queue_settled"])

    def test_path_arguments_and_changed_local_path_refused(self):
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {"bus": "/outside/bus.jsonl"})
        self.mailbox.args.bus = self.root.parent / "outside.jsonl"
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {})

    def test_symlink_escape_refused_before_read(self):
        with tempfile.TemporaryDirectory() as other:
            outside = Path(other) / "synthetic-sentinel"
            outside.write_text("synthetic-only sentinel")
            (self.root / "escape").symlink_to(outside)
            with patch.object(PING.D, "read_pending_command") as read, self.assertRaises(ValueError):
                self.mailbox.call("read_pending", {})
            read.assert_not_called()
            self.assertEqual(outside.read_text(), "synthetic-only sentinel")

    def test_marker_owner_tamper_refused(self):
        PING.D.atomic_json(self.root / "fixture.json", {
            "schema": "codescribe.synthetic-probe.v1", "session": "foreign", "lease_id": self.mailbox.lease})
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {})

    def test_timeout_cleans_only_owned_fixture(self):
        created = []
        def timed_out(root, *_args):
            created.append(root)
            self.assertTrue(root.exists())
            raise subprocess.TimeoutExpired("fixture", 0.05)
        with patch.object(PING.D, "reply_publisher_command", return_value=str(PING.PYTHON)), \
                patch.object(PING, "exchange", side_effect=timed_out), self.assertRaises(subprocess.TimeoutExpired):
            PING.run(0.05)
        self.assertEqual(len(created), 1)
        self.assertFalse(created[0].exists())
        self.assertTrue(self.root.exists())

    def test_deadline_kills_owned_process_group_without_retry(self):
        child = MagicMock(pid=424242)
        child.communicate.side_effect = subprocess.TimeoutExpired("fixture", 0.05)
        with patch.object(PING.subprocess, "Popen", return_value=child) as spawn, \
                patch.object(PING.os, "killpg") as kill, self.assertRaises(subprocess.TimeoutExpired):
            PING.exchange(self.root, self.session, self.identity, self.bus, PING.PYTHON, 0.05)
        spawn.assert_called_once()
        kill.assert_called_once_with(424242, signal.SIGKILL)
        child.wait.assert_called_once_with(timeout=2)
        self.assertEqual(spawn.call_args.kwargs["env"]["HOME"], str(self.root))
        self.assertNotIn("OPENAI_API_KEY", spawn.call_args.kwargs["env"])

    def test_timeout_bounds_and_no_unsandboxed_mode(self):
        for timeout in [0, 31]:
            with self.assertRaises(ValueError):
                PING.run(timeout)
        with patch.object(PING.sys, "platform", "linux"), patch.object(PING.subprocess, "Popen") as spawn:
            with self.assertRaises(RuntimeError):
                PING.exchange(self.root, self.session, self.identity, self.bus, PING.PYTHON, 1)
        spawn.assert_not_called()


if __name__ == "__main__":
    unittest.main()
