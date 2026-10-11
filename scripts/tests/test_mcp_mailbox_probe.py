"""Text MCP probe: exact read/ACK ownership and one canonical reply, no TTS."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[1] / "mcp-mailbox-probe.py"
SPEC = importlib.util.spec_from_file_location("mailbox_probe_test", SOURCE)
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
D = PROBE.DEMUX


class MailboxProbeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.session = "probe-fixture"
        self.lease = D.lease_identifier("codex", self.session)
        self.ids = [f"{i:024x}" for i in range(1, 6)]
        owner = {"provider": "codex", "provider_session_id": self.session, "lease_id": self.lease}
        self.pending = [dict(owner, delivery_owner=owner.copy(), delivery_id=identity,
                             kind="message", source="typed", text="Miksa, ping", audience="miksa",
                             bus=str(self.bus), emitted_at="2026-10-11T12:00:00Z", message_id=identity)
                        for identity in self.ids]
        self.save()
        self.mailbox = PROBE.Mailbox("codex", self.session, self.root, self.bus)

    def save(self):
        D.atomic_json(self.root / "leases" / f"{self.lease}.json", {
            "schema": D.LEASE_SCHEMA, "lease_id": self.lease, "provider": "codex",
            "provider_session_id": self.session, "bus": str(self.bus), "name": "miksa",
            "pending": self.pending, "cursor": 42})

    def read_ack(self):
        self.mailbox.call("read_pending", {})
        self.mailbox.call("ack", {"request_id": self.ids[0]})

    def reply(self, text="pong"):
        return self.mailbox.call("reply", {"request_id": self.ids[0], "text": text})

    def test_read_preserves_five_equal_deliveries_and_never_acks(self):
        self.mailbox.args.read_limit = 8
        before = (self.root / "leases" / f"{self.lease}.json").read_bytes()
        result = self.mailbox.call("read_pending", {})
        self.assertEqual([event["request_id"] for event in result["events"]], self.ids)
        self.assertEqual([event["delivery"] for event in result["events"]], self.pending)
        self.assertEqual({event["event"] for event in result["events"]}, {"transcript.ready"})
        self.assertEqual(before, (self.root / "leases" / f"{self.lease}.json").read_bytes())
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_unknown_unread_foreign_and_missing_arguments_refused(self):
        for name, args in [("ack", {"request_id": self.ids[0]}),
                           ("reply", {"request_id": self.ids[0], "text": "pong"}),
                           ("read_pending", {"session": "foreign"}), ("ack", {})]:
            with self.subTest(name=name, args=args), self.assertRaises(ValueError):
                self.mailbox.call(name, args)
        self.pending[0]["delivery_owner"]["provider_session_id"] = "foreign"
        self.save()
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {})

    def test_reply_requires_read_then_ack_and_preserves_owner_after_ack(self):
        self.mailbox.call("read_pending", {})
        with self.assertRaises(ValueError):
            self.reply()
        self.mailbox.call("ack", {"request_id": self.ids[0]})
        with patch.object(D, "publish_reply_event", return_value={
                "stream_dev": 0, "stream_inode": 1, "stream_id": "fixture", "offset": 0, "length": 10}) as publish, \
                patch.object(D, "speak_published_reply", side_effect=AssertionError("speech forbidden")):
            receipt = self.reply()
            self.assertEqual(self.reply(), receipt)
            publish.assert_called_once()
            envelope = publish.call_args.args[1]
            self.assertEqual(envelope["delivery_id"], self.ids[0])
            self.assertEqual(envelope["provider_session_id"], self.session)
            self.assertEqual(envelope["text"], "pong")
            self.assertFalse(envelope["spoken"])
            with self.assertRaises(ValueError):
                self.reply("different")

    def test_ambiguous_publication_is_never_blindly_retried(self):
        self.read_ack()
        with patch.object(D, "publish_reply_event", side_effect=OSError("ambiguous")) as publish:
            with self.assertRaises(OSError):
                self.reply()
            with self.assertRaisesRegex(ValueError, "uncertain"):
                self.reply()
            publish.assert_called_once()

    def test_rebound_bus_and_invalid_text_refused_before_publish(self):
        self.read_ack()
        for text in ["", " ", "ą" * 40000, 2]:
            with self.assertRaises(ValueError):
                self.reply(text)
        state = D.read_json(self.root / "leases" / f"{self.lease}.json")
        state["bus"] = str(self.root / "foreign.jsonl")
        D.atomic_json(self.root / "leases" / f"{self.lease}.json", state)
        with patch.object(D, "publish_reply_event") as publish, self.assertRaises(ValueError):
            self.reply()
        publish.assert_not_called()

    def test_rpc_tools_only_and_notifications_never_write(self):
        self.assertIsNone(PROBE.dispatch(self.mailbox, {
            "jsonrpc": "2.0", "method": "tools/call", "params": {
                "name": "ack", "arguments": {"request_id": self.ids[0]}}}))
        result = PROBE.dispatch(self.mailbox, {"jsonrpc": "2.0", "id": 1, "method": "initialize"})
        self.assertEqual(result["result"]["capabilities"], {"tools": {}})
        unknown = PROBE.dispatch(self.mailbox, {"jsonrpc": "2.0", "id": 2, "method": "events/subscribe"})
        self.assertEqual(unknown["error"]["code"], -32601)

    def test_real_stdio_text_ping_uses_existing_rust_publisher(self):
        # No compilation or installation: exercise the already installed canonical CLI.
        D.reply_publisher_command(self.root)
        self.pending = self.pending[:1]
        self.save()
        calls = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "read_pending", "arguments": {}}},
            {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "ack", "arguments": {"request_id": self.ids[0]}}},
            {"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "read_pending", "arguments": {}}},
            {"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "reply", "arguments": {"request_id": self.ids[0], "text": "pong"}}},
        ]
        result = subprocess.run([sys.executable, "-B", str(SOURCE), "--provider", "codex",
                                 "--session", self.session, "--bridge-home", str(self.root),
                                 "--bus", str(self.bus)],
                                input="".join(json.dumps(row) + "\n" for row in calls),
                                capture_output=True, text=True, timeout=45, check=True)
        responses = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(len(responses), 6)
        for response in responses:
            self.assertFalse(response.get("result", {}).get("isError"), response)
        receipt = json.loads(responses[-1]["result"]["content"][0]["text"])
        bus, reply = D.load_published_reply(self.mailbox.args, receipt["reply_id"])
        self.assertEqual(bus, self.bus)
        self.assertEqual(reply["delivery_id"], self.ids[0])
        self.assertEqual(reply["text"], "pong")
        self.assertFalse(reply["spoken"])
        self.assertEqual((self.bus.stat().st_mode & 0o777), 0o600)
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
