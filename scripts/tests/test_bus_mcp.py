"""Native MCP reads owned bus deliveries; only an explicit ACK consumes them."""
import contextlib
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
DEMUX_SPEC = importlib.util.spec_from_file_location("bus_mcp_demux_tests", SCRIPTS / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(DEMUX_SPEC)
sys.modules[DEMUX_SPEC.name] = DEMUX
DEMUX_SPEC.loader.exec_module(DEMUX)
MCP_SPEC = importlib.util.spec_from_file_location("bus_mcp_tests", SCRIPTS / "bus-mcp.py")
MCP = importlib.util.module_from_spec(MCP_SPEC)
sys.modules[MCP_SPEC.name] = MCP
MCP_SPEC.loader.exec_module(MCP)


def parse_wake_notice(line):
    """The wake is one English sentence (what the model reads); recover its facts."""
    text = line.split("Codescribe: ", 1)[1]
    head = re.search(r"for (?P<name>\S+) on channel (?P<channel>\d) \((?P<provider>\S+) session (?P<session>[^)]+)\)", text)
    notice = {"name": head["name"], "channel": head["channel"], "provider": head["provider"],
              "provider_session_id": head["session"]}
    if text.startswith("listener problem "):
        notice.update(kind="codescribe_listener_problem", problem=text.split(" ")[2])
    elif text.startswith("no message "):
        notice["kind"] = "codescribe_wait_idle"
    else:
        notice.update(kind="codescribe_mailbox_ready", pending_count=int(re.search(r"pending (\d+)", text)[1]),
                      new_delivery_ids=re.search(r"new delivery ids: ([^.]*)\.", text)[1].split(", "))
        problem = re.search(r"problem: (\S+?);", text)
        if problem:
            notice["problem"] = problem[1]
    return notice


class MailboxFixture(unittest.TestCase):
    PROVIDER = "junie"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix=".bus-mcp-test-", dir=Path(__file__).parent)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.session = "11111111-2222-4333-8444-555555555555"
        self.lease = DEMUX.lease_identifier(self.PROVIDER, self.session)
        self.lease_path = self.root / "leases" / f"{self.lease}.json"
        self.ids = [f"{n:024x}" for n in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        self.save()
        self.mailbox = MCP.Mailbox(DEMUX, self.root, self.bus)
        self.mailbox.owner, self.mailbox.provider = self.session, self.PROVIDER

    def envelope(self, identity):
        owner = {"lease_id": self.lease, "provider": self.PROVIDER, "provider_session_id": self.session}
        return dict(owner, delivery_owner=dict(owner), delivery_id=identity,
                    kind="message", source="typed", text="Iwo", bus=str(self.bus),
                    emitted_at="2026-10-11T19:03:00Z", message_id=identity)

    def save(self):
        DEMUX.atomic_json(self.lease_path, {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease, "provider": self.PROVIDER,
            "provider_session_id": self.session, "bus": str(self.bus),
            "pending": self.pending, "cursor": 0,
        })

    def call(self, name, values=None, cancel=None):
        return self.mailbox.call(f"codescribe_{name}", {} if values is None else values,
                                 threading.Event() if cancel is None else cancel)

    def assert_not_acknowledged(self):
        self.assertEqual(self.mailbox.acked_ids, set())
        self.assertFalse((self.root / "acknowledgments").exists())

    def start_wait(self):
        cancel = threading.Event()
        outcome = queue.Queue()

        def wait():
            try:
                outcome.put(("result", self.call("wait", cancel=cancel)))
            except Exception as error:
                outcome.put(("error", error))

        thread = threading.Thread(target=wait, daemon=True)
        thread.start()

        def stop():
            cancel.set()
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive(), "mailbox waiter did not stop after cancellation")

        self.addCleanup(stop)
        deadline = time.monotonic() + 3
        while self.mailbox.active_wait is None and thread.is_alive() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertIsNotNone(self.mailbox.active_wait, "waiter never became active")
        return cancel, thread, outcome


class MailboxTests(MailboxFixture):
    def test_read_returns_complete_owned_envelope_without_ack(self):
        self.pending[0]["text"] = "Iwo — cała wiadomość, nie podgląd. " * 100
        self.pending[0]["attachments"] = [{"path": str(self.root / "draft.txt"), "name": "draft.txt",
                                            "media_type": "text/plain", "bytes": 4, "sha256": "a" * 64}]
        self.save()
        before = self.lease_path.read_bytes()
        result = self.call("read")
        self.assertEqual(result["deliveries"], [
            {**row, "addressed_to": "unknown (source has no address)"} for row in self.pending[:1]])
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(result["remaining"], 4)
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.call("read"), result)
        self.assertEqual(self.lease_path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_wait_returns_existing_message_without_ack(self):
        before = self.lease_path.read_bytes()
        result = self.call("wait")
        self.assertEqual(result["deliveries"], [
            {**row, "addressed_to": "unknown (source has no address)"} for row in self.pending[:1]])
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.lease_path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_five_identical_iwo_remain_five_distinct_reads_and_acks(self):
        read_ids = []
        ack_ids = []
        for index, identity in enumerate(self.ids):
            result = self.call("read")
            self.assertEqual(result["deliveries"], [
                {**self.pending[index], "addressed_to": "unknown (source has no address)"}])
            self.assertEqual(result["read_delivery_ids"], [identity])
            self.assertEqual(result["remaining"], 4 - index)
            read_ids.extend(result["read_delivery_ids"])
            self.assertFalse((self.root / "acknowledgments" / self.lease / f"{identity}.json").exists())
            receipt = self.call("ack", {"delivery_ids": result["read_delivery_ids"]})
            self.assertEqual(receipt["kind"], "acknowledged")
            self.assertTrue(receipt["native_queue_settled"])
            ack_ids.append(receipt["delivery_id"])
            marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{identity}.json")
            self.assertEqual(marker["delivery_id"], identity)
            self.assertEqual(marker["provider_session_id"], self.session)
        self.assertEqual(read_ids, self.ids)
        self.assertEqual(ack_ids, self.ids)
        self.assertEqual(self.mailbox.read_ids, set(self.ids))
        self.assertEqual(self.mailbox.acked_ids, set(self.ids))
        self.assertEqual(len(list((self.root / "acknowledgments" / self.lease).iterdir())), 5)
        self.assertEqual(self.call("read")["deliveries"], [])

    def test_ack_requires_read_and_refuses_whole_mixed_batch(self):
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids})
        self.call("read")
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:2]})
        self.assert_not_acknowledged()

    def test_ack_is_idempotent_without_rewriting_read_receipt(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        marker = self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json"
        before = marker.read_bytes()
        self.call("ack", {"delivery_ids": self.ids[:1]})
        self.assertEqual(marker.read_bytes(), before)
        self.assertEqual(self.mailbox.acked_ids, {self.ids[0]})

    def test_read_refuses_foreign_owner_even_beyond_first_envelope(self):
        for field in ("provider", "provider_session_id", "lease_id"):
            with self.subTest(field=field):
                self.pending = [self.envelope(identity) for identity in self.ids]
                self.pending[-1]["delivery_owner"][field] = "foreign"
                self.save()
                with self.assertRaisesRegex(ValueError, "foreign pending delivery owner"):
                    self.call("read")
                self.assertEqual(self.mailbox.read_ids, set())
                self.assert_not_acknowledged()

    def test_wait_refuses_foreign_envelope_without_marking_it_read(self):
        self.pending[0]["provider_session_id"] = "foreign"
        self.save()
        with self.assertRaisesRegex(ValueError, "foreign"):
            self.call("wait")
        self.assertIsNone(self.mailbox.active_wait)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_ack_rechecks_lease_owner_after_read(self):
        self.call("read")
        state = DEMUX.read_json(self.lease_path)
        state["provider_session_id"] = "foreign"
        DEMUX.atomic_json(self.lease_path, state)
        with self.assertRaisesRegex(ValueError, "does not belong"):
            self.call("ack", {"delivery_ids": self.ids[:1]})
        self.assert_not_acknowledged()

    def test_oversized_read_and_wait_refuse_without_read_or_ack(self):
        self.pending[0]["text"] = "Iwo " * 300000
        self.save()
        for operation in ("read", "wait"):
            with self.subTest(operation=operation), self.assertRaisesRegex(ValueError, "read-bytes"):
                self.call(operation)
            self.assertEqual(self.mailbox.read_ids, set())
            self.assert_not_acknowledged()
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:1]})

    def test_wait_remains_pending_until_future_arrival_without_ack(self):
        self.pending = []
        self.save()
        _cancel, thread, outcome = self.start_wait()
        with self.assertRaises(queue.Empty):
            outcome.get(timeout=0.2)
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        kind, result = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(kind, "result", result)
        self.assertEqual(result["deliveries"], [
            {**row, "addressed_to": "unknown (source has no address)"} for row in self.pending])
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assert_not_acknowledged()

    def test_cancel_wait_releases_waiter_without_read_or_ack(self):
        self.pending = []
        self.save()
        cancel, thread, outcome = self.start_wait()
        cancel.set()
        kind, error = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(kind, "error")
        self.assertIsInstance(error, ValueError)
        self.assertIn("cancel", str(error))
        self.assertFalse(thread.is_alive())
        self.assertIsNone(self.mailbox.active_wait)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        self.assertEqual(self.call("wait")["read_delivery_ids"], self.ids[:1])

    def test_already_cancelled_wait_does_not_read_pending_message(self):
        cancel = threading.Event()
        cancel.set()
        with self.assertRaisesRegex(ValueError, "cancel"):
            self.call("wait", cancel=cancel)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_newer_wait_supersedes_abandoned_wait_without_read_or_ack(self):
        # J-01: Junie abandons a wait at its deadline and never cancels it.
        self.pending = []
        self.save()
        _cancel, thread, outcome = self.start_wait()
        orphan = self.mailbox.active_wait
        newer = queue.Queue()
        threading.Thread(target=lambda: newer.put(self.call("wait")), daemon=True).start()
        kind, error = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(kind, "error")
        self.assertIn("superseded", str(error))
        deadline = time.monotonic() + 3
        while self.mailbox.active_wait in (None, orphan) and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertNotIn(self.mailbox.active_wait, (None, orphan))
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        self.assertEqual(newer.get(timeout=5)["read_delivery_ids"], self.ids[:1])
        self.assert_not_acknowledged()

    def test_wait_window_ends_with_explicit_idle_receipt(self):
        self.pending = []
        self.save()
        started = time.monotonic()
        result = self.call("wait", {"timeout_seconds": 1})
        self.assertLess(time.monotonic() - started, 4)
        self.assertEqual(result["kind"], "wait_idle")
        self.assertEqual((result["provider"], result["provider_session_id"]), (self.PROVIDER, self.session))
        self.assertEqual(result["read_delivery_ids"], [])
        self.assertFalse(result["acknowledged"])
        self.assertIn("codescribe_wait again", result["instructions"])
        self.assertIsNone(self.mailbox.active_wait)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_wait_window_stays_below_the_host_deadline(self):
        window = MCP.WAIT_SECONDS[self.PROVIDER]
        for seconds in (0, window + 1):
            with self.subTest(seconds=seconds), self.assertRaisesRegex(ValueError, "timeout_seconds"):
                self.call("wait", {"timeout_seconds": seconds})
        with self.assertRaisesRegex(ValueError, "type"):
            self.call("wait", {"timeout_seconds": True})
        self.assertIsNone(self.mailbox.active_wait)

    def test_detach_stops_active_wait_and_preserves_mailbox(self):
        self.pending = []
        self.save()
        _cancel, thread, outcome = self.start_wait()
        result = self.call("detach")
        kind, error = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(result["kind"], "detach_receipt")
        self.assertEqual(kind, "error")
        self.assertIn("detach", str(error))
        self.assertIsNone(self.mailbox.owner)
        self.assert_not_acknowledged()

    def test_connect_cannot_change_owner_or_receipt_sets(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        with self.assertRaisesRegex(ValueError, "another session"):
            self.call("connect", {"provider": self.PROVIDER, "session_id": "other-native-session", "name": "Roman", "channel": "3"})
        other = next(name for name in MCP.WAIT_SECONDS if name != self.PROVIDER)
        with self.assertRaisesRegex(ValueError, "another session"):
            self.call("connect", {"provider": other, "session_id": self.session, "name": "Roman", "channel": "3"})
        self.assertEqual((self.mailbox.provider, self.mailbox.owner), (self.PROVIDER, self.session))
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.mailbox.acked_ids, {self.ids[0]})

    def test_connect_validates_identity_name_and_channel_before_side_effects(self):
        self.mailbox.owner = self.mailbox.provider = None
        valid = {"provider": self.PROVIDER, "session_id": self.session, "name": "Roman", "channel": "3"}
        for field, value in (("provider", "claude"), ("provider", "codex"), ("provider", None),
                             ("session_id", ""), ("session_id", "\n"), ("session_id", 3),
                             ("name", " "), ("name", None), ("channel", "0"),
                             ("channel", "10"), ("channel", 3)):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.call("connect", dict(valid, **{field: value}))
            self.assertIsNone(self.mailbox.owner)
        with self.assertRaises(ValueError):
            self.call("connect", {key: value for key, value in valid.items() if key != "provider"})
        self.assertFalse((self.root / DEMUX.AUDIENCE_BINDING_FILENAME).exists())

    def test_failed_connect_keeps_existing_slot_and_restores_unconnected_owner(self):
        self.mailbox.owner = self.mailbox.provider = None
        foreign = {"audience": "someone-else", "provider": self.PROVIDER,
                   "provider_session_id": "foreign", "bus": str(self.bus)}
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings["3"] = foreign
        path = self.root / DEMUX.AUDIENCE_BINDING_FILENAME
        before = path.read_bytes()
        with self.assertRaises(ValueError):
            self.call("connect", {"provider": self.PROVIDER, "session_id": self.session, "name": "Roman", "channel": "3"})
        self.assertIsNone(self.mailbox.owner)
        self.assertIsNone(self.mailbox.provider)
        self.assertEqual(path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_all_session_tools_refuse_before_connect(self):
        self.mailbox.owner = None
        operations = [("read", {}), ("wait", {}), ("status", {}), ("detach", {}),
                      ("ack", {"delivery_ids": self.ids[:1]}),
                      ("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})]
        for name, values in operations:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "connect"):
                self.call(name, values)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_invalid_tool_arguments_do_not_change_receipts(self):
        operations = [("read", {"session_id": "foreign"}), ("wait", {"timeout": 1}),
                      ("ack", {}), ("ack", {"delivery_ids": []}),
                      ("ack", {"delivery_ids": self.ids[0]}), ("reply", {"text": "Odpowiedź"}),
                      ("unknown", {}), ("status", [])]
        for name, values in operations:
            with self.subTest(name=name, values=values), self.assertRaises(ValueError):
                self.call(name, values)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assertEqual(self.mailbox.replied_ids, set())
        self.assert_not_acknowledged()

    def test_no_bus_override_reads_lease_bus_without_touching_default_bus(self):
        self.mailbox = MCP.Mailbox(DEMUX, self.root, None)
        self.mailbox.owner, self.mailbox.provider = self.session, self.PROVIDER
        self.assertEqual(self.call("read")["deliveries"], [
            {**row, "addressed_to": "unknown (source has no address)"} for row in self.pending[:1]])
        self.call("ack", {"delivery_ids": self.ids[:1]})
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json")
        self.assertEqual(marker["bus"], str(self.bus))

    def test_status_observes_backlog_without_read_or_ack(self):
        result = self.call("status")
        self.assertEqual(result["kind"], "status")
        self.assertEqual(result["lease_id"], self.lease)
        self.assertEqual(result["backlog"], 5)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_read_wait_and_status_report_dynamic_mute_snapshot_from_lease_bus(self):
        self.mailbox.bus = None
        global_path = self.root / "runtime/playback-mutes/global.json"
        agent_path = DEMUX.playback_mute_path(self.root, self.PROVIDER, self.session, self.bus)
        for global_muted, agent_muted in ((False, False), (False, True), (True, True), (True, False)):
            with self.subTest(global_muted=global_muted, agent_muted=agent_muted):
                DEMUX.atomic_json(global_path, {"schema": DEMUX.PLAYBACK_MUTE_SCHEMA,
                                               "scope": "all", "muted": global_muted})
                DEMUX.atomic_json(agent_path, {"schema": DEMUX.PLAYBACK_MUTE_SCHEMA,
                                              "provider": self.PROVIDER, "provider_session_id": self.session,
                                              "lease_id": self.lease, "bus": str(self.bus), "muted": agent_muted})
                expected = {"global_muted": global_muted, "agent_muted": agent_muted,
                            "effective_muted": global_muted or agent_muted,
                            "mute_scope": "global" if global_muted else "agent" if agent_muted else None}
                before = {path: path.read_bytes() for path in (global_path, agent_path, self.lease_path)}
                for operation in ("read", "wait", "status"):
                    with self.subTest(operation=operation):
                        result = self.call(operation)
                        self.assertEqual(result["playback"], expected)
                self.assertEqual({path: path.read_bytes() for path in before}, before)
                self.assert_not_acknowledged()

    def test_wait_idle_reports_global_mute_without_read_or_ack(self):
        self.pending = []
        self.save()
        self.mailbox.bus = None
        global_path = self.root / "runtime/playback-mutes/global.json"
        DEMUX.atomic_json(global_path, {"schema": DEMUX.PLAYBACK_MUTE_SCHEMA, "scope": "all", "muted": True})
        before = self.lease_path.read_bytes()
        result = self.call("wait", {"timeout_seconds": 1})
        self.assertEqual(result["kind"], "wait_idle")
        self.assertEqual(result["playback"], {"global_muted": True, "agent_muted": False,
                                            "effective_muted": True, "mute_scope": "global"})
        self.assertEqual(self.lease_path.read_bytes(), before)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_detach_preserves_unread_deliveries_without_ack(self):
        result = self.call("detach")
        self.assertEqual(result["kind"], "detach_receipt")
        self.assertEqual(result["unacked_deliveries"], 5)
        self.assertEqual(result["unacked_delivery_ids"], self.ids)
        self.assertIsNone(self.mailbox.owner)
        self.assertEqual(DEMUX.read_json(self.lease_path)["pending"], self.pending)
        self.assert_not_acknowledged()

    def test_detach_drained_mailbox_requires_connect_again(self):
        self.pending = []
        self.save()
        result = self.call("detach")
        self.assertEqual(result["kind"], "detach_receipt")
        self.assertEqual(result["unacked_deliveries"], 0)
        self.assertIsNone(self.mailbox.owner)
        with self.assertRaisesRegex(ValueError, "connect"):
            self.call("read")

    def test_reply_requires_ack_not_just_read(self):
        values = {"delivery_id": self.ids[0], "text": "Odpowiedź"}
        with self.assertRaisesRegex(ValueError, "acknowledged"):
            self.call("reply", values)
        self.call("read")
        with self.assertRaisesRegex(ValueError, "acknowledged"):
            self.call("reply", values)
        self.assertEqual(self.mailbox.replied_ids, set())
        self.assert_not_acknowledged()

    def test_reply_speaks_once_using_exact_acked_id_and_owned_root(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        text = "Gotowe — pełna odpowiedź."
        receipt = {"kind": "voice_reply", "delivery_id": self.ids[0], "spoken": True}
        with patch.object(MCP.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 0, json.dumps(receipt), "")) as tts:
            self.assertEqual(self.call("reply", {"delivery_id": self.ids[0], "text": text}), receipt)
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": text})
        tts.assert_called_once()
        command = tts.call_args.args[0]
        self.assertEqual(command[:2], [sys.executable, str(SCRIPTS / "cs-bus")])
        self.assertEqual(command[command.index("--bridge-home") + 1], str(self.root))
        self.assertEqual(command[command.index("--bus") + 1], str(self.bus))
        self.assertEqual(command[command.index("--provider") + 1], self.PROVIDER)
        self.assertEqual(command[command.index("--session") + 1], self.session)
        self.assertEqual(command[-4:], ["--say", text, "--reply-to", self.ids[0]])
        self.assertEqual(self.mailbox.replied_ids, {self.ids[0]})

    def test_failed_voice_reply_keeps_receipt_and_cannot_be_replayed(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        receipt = {"kind": "voice_reply", "spoken": False, "reason": "recording_active"}
        with patch.object(MCP.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 3, json.dumps(receipt), "playback refused")) as tts:
            result = self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
            self.assertEqual(result, {"receipts": [receipt], "exit_code": 3})
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
        tts.assert_called_once()

    def test_tts_timeout_is_an_attempt_not_permission_to_reply_again(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        with patch.object(MCP.subprocess, "run", side_effect=subprocess.TimeoutExpired("tts", 300)) as tts:
            with self.assertRaises(subprocess.TimeoutExpired):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
        tts.assert_called_once()
        self.assertEqual(self.mailbox.replied_ids, {self.ids[0]})

    def test_invalid_reply_text_does_not_consume_attempt(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        for text in ("", " \n", None, 1):
            error = "text" if isinstance(text, str) else "type"
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, error):
                self.call("reply", {"delivery_id": self.ids[0], "text": text})
        self.assertEqual(self.mailbox.replied_ids, set())


class ExtensionInstallTests(MailboxFixture):
    def setUp(self):
        super().setUp()
        self.source = self.root / "runtime" / "plugins"
        marketplace = self.source / ".claude-plugin" / "marketplace.json"
        DEMUX.atomic_json(marketplace, {"name": "codescribe", "plugins": [{"name": "codescribe", "source": "./codescribe"}]})
        self.directory = self.root / "extensions"
        self.target = self.directory / "extensions.json"
        self.identity = "native-registered-marketplace"

    def install(self):
        return MCP.install_extension(DEMUX, self.root, self.directory)

    def test_install_preserves_foreign_entries_and_is_idempotent(self):
        foreign = {"remote-other": {"source": {"type": "git", "url": "https://example.invalid/plugins"},
                                    "extensions": ["other"], "custom": {"keep": True}}}
        registered = {"source": {"type": "local", "path": str(self.source)}, "extensions": []}
        DEMUX.atomic_json(self.target, {**foreign, self.identity: registered})
        receipt = self.install()
        config = DEMUX.read_json(self.target)
        self.assertEqual(receipt["kind"], "extension_registered")
        self.assertEqual(receipt["source"], str(self.source))
        self.assertEqual(receipt["configuration"], str(self.target))
        self.assertEqual(config["remote-other"], foreign["remote-other"])
        self.assertEqual(config[self.identity], {"source": {"type": "local", "path": str(self.source)},
                                               "extensions": ["codescribe"]})
        before = self.target.read_bytes()
        self.assertEqual(self.install(), receipt)
        self.assertEqual(self.target.read_bytes(), before)

    def test_unregistered_marketplace_requires_native_registration_without_inventing_identity(self):
        with self.assertRaisesRegex(ValueError, "/extensions marketplace add"):
            self.install()
        self.assertFalse(self.target.exists())

    def test_install_appends_without_replacing_existing_extension_metadata(self):
        entry = {"source": {"type": "local", "path": str(self.source)},
                 "extensions": ["another"], "setting": "retained"}
        DEMUX.atomic_json(self.target, {self.identity: entry})
        self.install()
        self.assertEqual(DEMUX.read_json(self.target)[self.identity], dict(entry, extensions=["another", "codescribe"]))

    def test_corrupt_configuration_is_refused_and_preserved_byte_for_byte(self):
        self.directory.mkdir()
        for raw in ("{not json", "[]", "null", "42", json.dumps({self.identity: []}),
                    json.dumps({self.identity: {"source": {"type": "local", "path": str(self.source)}}}),
                    json.dumps({self.identity: {"source": {"type": "local", "path": str(self.source)}, "extensions": [1]}})):
            with self.subTest(raw=raw):
                self.target.write_text(raw)
                before = self.target.read_bytes()
                with self.assertRaises(ValueError):
                    self.install()
                self.assertEqual(self.target.read_bytes(), before)

    def test_conflicting_marketplace_source_is_refused_without_overwrite(self):
        DEMUX.atomic_json(self.target, {self.identity: {"source": {"type": "local", "path": "/unrelated"},
                                                       "extensions": ["another"]}})
        before = self.target.read_bytes()
        with self.assertRaisesRegex(ValueError, "conflict"):
            self.install()
        self.assertEqual(self.target.read_bytes(), before)

    def test_missing_payload_does_not_create_configuration(self):
        (self.source / ".claude-plugin" / "marketplace.json").unlink()
        with self.assertRaisesRegex(ValueError, "payload missing"):
            self.install()
        self.assertFalse(self.target.exists())

    def test_symlinked_configuration_is_refused_without_touching_its_target(self):
        outside = self.root / "unrelated.json"
        DEMUX.atomic_json(outside, {"unrelated": True})
        self.directory.mkdir()
        self.target.symlink_to(outside)
        before = outside.read_bytes()
        with self.assertRaises(ValueError):
            self.install()
        self.assertTrue(self.target.is_symlink())
        self.assertEqual(outside.read_bytes(), before)

    def test_concurrent_installations_preserve_both_local_sources(self):
        other_root = self.root / "second-bridge"
        other_source = other_root / "runtime" / "plugins"
        DEMUX.atomic_json(other_source / ".claude-plugin" / "marketplace.json", {"name": "codescribe", "plugins": []})
        DEMUX.atomic_json(self.target, {"foreign": {"retained": True},
                                      self.identity: {"source": {"type": "local", "path": str(self.source)}, "extensions": []},
                                      "native-second-marketplace": {"source": {"type": "local", "path": str(other_source)}, "extensions": []}})
        start = threading.Barrier(3)
        outcomes = queue.Queue()

        def install(root):
            start.wait(timeout=5)
            try:
                outcomes.put(MCP.install_extension(DEMUX, root, self.directory))
            except Exception as error:
                outcomes.put(error)

        threads = [threading.Thread(target=install, args=(root,)) for root in (self.root, other_root)]
        for thread in threads:
            thread.start()
        start.wait(timeout=5)
        for thread in threads:
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
        for _ in threads:
            result = outcomes.get(timeout=1)
            self.assertIsInstance(result, dict, result)
        config = DEMUX.read_json(self.target)
        self.assertEqual(config["foreign"], {"retained": True})
        sources = {entry["source"]["path"] for key, entry in config.items() if key != "foreign"}
        self.assertEqual(sources, {str(self.source), str(other_source)})
        self.assertEqual(len(config), 3)


class MCPProtocolTests(MailboxFixture):
    def start_server(self, override_bus=True):
        self.pending = []
        self.save()
        command = [sys.executable, str(SCRIPTS / "cs-bus"), "--mcp", "--bridge-home", str(self.root)]
        if override_bus:
            command += ["--bus", str(self.bus)]
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)
        self.responses = queue.Queue()
        self.stderr = []

        def read_stdout():
            for line in self.process.stdout:
                self.responses.put(line)

        def read_stderr():
            self.stderr.extend(self.process.stderr)

        self.readers = [threading.Thread(target=read_stdout, daemon=True),
                        threading.Thread(target=read_stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        self.addCleanup(self.stop_server)
        self.addCleanup(self.detach_follower)

    def detach_follower(self):
        result = subprocess.run([sys.executable, str(SCRIPTS / "cs-bus"), "--detach",
                                 "--bridge-home", str(self.root), "--provider", self.PROVIDER,
                                 "--session", self.session], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def stop_server(self):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
            self.fail("MCP process did not exit after stdin EOF")
        finally:
            for reader in self.readers:
                reader.join(timeout=2)
            self.process.stdout.close()
            self.process.stderr.close()

    def send(self, method, identity=None, params=None):
        request = {"jsonrpc": "2.0", "method": method}
        if identity is not None:
            request["id"] = identity
        if params is not None:
            request["params"] = params
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()

    def response(self, identity, timeout=10):
        try:
            line = self.responses.get(timeout=timeout)
        except queue.Empty:
            self.fail(f"no MCP response for {identity}; stderr: {''.join(self.stderr)}")
        value = json.loads(line)
        self.assertEqual(value["jsonrpc"], "2.0")
        self.assertEqual(value["id"], identity, value)
        return value

    def tool(self, identity, name, arguments=None):
        self.send("tools/call", identity, {"name": f"codescribe_{name}",
                                          "arguments": {} if arguments is None else arguments})
        result = self.response(identity)["result"]
        self.assertFalse(result.get("isError"), result)
        self.assertEqual(result["content"][0]["type"], "text")
        text = result["content"][0]["text"]
        self.assertTrue(text.strip() and not text.lstrip().startswith("{"), text)
        return result["structuredContent"]

    def connect(self):
        result = self.tool(10, "connect", {"provider": self.PROVIDER, "session_id": self.session,
                                           "name": "Roman", "channel": "3"})
        self.assertEqual(result["kind"], "attach_receipt")
        self.assertEqual(result["provider"], self.PROVIDER)
        self.assertEqual(result["provider_session_id"], self.session)
        self.assertEqual(result["wakeup"], "off")
        return result

    def start_active_wait(self, identity):
        self.send("tools/call", identity, {"name": "codescribe_wait", "arguments": {}})
        with self.assertRaises(queue.Empty):
            self.responses.get(timeout=0.2)

    def test_initialize_and_list_advertise_all_seven_native_tools(self):
        self.start_server(override_bus=False)
        self.send("initialize", 1, {"protocolVersion": "2024-11-05", "capabilities": {},
                                    "clientInfo": {"name": "hermetic-test", "version": "1"}})
        result = self.response(1)["result"]
        self.assertEqual(result["protocolVersion"], "2024-11-05")
        self.assertEqual(result["serverInfo"]["name"], "codescribe")
        self.assertEqual(result["capabilities"]["tools"], {})
        self.send("notifications/initialized")
        self.send("tools/list", 2)
        tools = self.response(2)["result"]["tools"]
        self.assertEqual({tool["name"] for tool in tools}, {f"codescribe_{name}" for name in
                         ("connect", "wait", "read", "ack", "reply", "status", "detach")})
        for tool in tools:
            self.assertEqual(tool["inputSchema"]["type"], "object")
            self.assertFalse(tool["inputSchema"]["additionalProperties"])
        self.assert_not_acknowledged()

    def test_tools_call_requires_connection_and_survives_bad_method(self):
        self.start_server()
        self.send("tools/call", 1, {"name": "codescribe_read", "arguments": {}})
        result = self.response(1)["result"]
        self.assertTrue(result["isError"])
        self.assertIn("connect", result["content"][0]["text"])
        self.send("no-such-method", 2)
        self.assertEqual(self.response(2)["error"]["code"], -32601)
        self.send("ping", 3)
        self.assertEqual(self.response(3)["result"], {})
        self.assert_not_acknowledged()

    def test_connect_read_status_detach_use_real_bus_subprocesses(self):
        self.start_server()
        self.connect()
        result = self.tool(11, "read")
        self.assertEqual(result["deliveries"], [])
        self.assertEqual(result["read_delivery_ids"], [])
        status = self.tool(12, "status")
        self.assertEqual(status["lease_id"], self.lease)
        self.assertEqual(status["channel"], "3")
        self.assertEqual(status["backlog"], 0)
        detach = self.tool(13, "detach")
        self.assertEqual(detach["released_channels"], ["3"])
        self.send("tools/call", 14, {"name": "codescribe_read", "arguments": {}})
        self.assertTrue(self.response(14)["result"]["isError"])
        self.assert_not_acknowledged()

    def test_new_connection_reports_global_mute_before_any_voice_attempt(self):
        self.start_server(override_bus=False)
        global_path = self.root / "runtime/playback-mutes/global.json"
        DEMUX.atomic_json(global_path, {"schema": DEMUX.PLAYBACK_MUTE_SCHEMA, "scope": "all", "muted": True})
        before = global_path.read_bytes()
        expected = {"global_muted": True, "agent_muted": False,
                    "effective_muted": True, "mute_scope": "global"}
        self.assertEqual(self.connect()["playback"], expected)
        for identity, operation in ((11, "read"), (12, "status")):
            self.assertEqual(self.tool(identity, operation)["playback"], expected)
        self.assertEqual(self.tool(13, "wait", {"timeout_seconds": 1})["playback"], expected)
        self.assertEqual(global_path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_cancel_notification_releases_wait_without_ack_or_late_result(self):
        self.start_server()
        self.connect()
        self.start_active_wait(20)
        self.send("notifications/cancelled", params={"requestId": 20, "reason": "test cancellation"})
        self.send("ping", 30)
        self.assertEqual(self.response(30)["result"], {})
        with self.assertRaises(queue.Empty):
            self.responses.get(timeout=0.7)
        self.start_active_wait(40)
        self.send("notifications/cancelled", params={"requestId": 40})
        self.process.stdin.close()
        self.assertEqual(self.process.wait(timeout=5), 0)
        for reader in self.readers:
            reader.join(timeout=2)
        self.assertTrue(self.responses.empty(), "cancelled waits emitted a late response")
        self.assert_not_acknowledged()

    def test_eof_cancels_pending_wait_and_exits_without_ack(self):
        self.start_server()
        self.connect()
        self.start_active_wait(20)
        self.process.stdin.close()
        self.assertEqual(self.process.wait(timeout=5), 0)
        for reader in self.readers:
            reader.join(timeout=2)
        self.assertTrue(self.responses.empty(), "EOF produced a cancelled wait result")
        self.assert_not_acknowledged()


class MCPWaitProtocolTests(MCPProtocolTests):
    def test_newer_wait_answers_the_abandoned_wait_and_stays_pending(self):
        self.start_server()
        self.connect()
        self.start_active_wait(20)
        self.send("tools/call", 21, {"name": "codescribe_wait", "arguments": {}})
        result = self.response(20)["result"]
        self.assertTrue(result["isError"])
        self.assertIn("superseded", result["content"][0]["text"])
        with self.assertRaises(queue.Empty):
            self.responses.get(timeout=0.3)
        self.send("notifications/cancelled", params={"requestId": 21})
        self.send("ping", 22)
        self.assertEqual(self.response(22)["result"], {})
        self.assert_not_acknowledged()

    def test_bounded_wait_returns_idle_receipt_over_stdio(self):
        self.start_server()
        self.connect()
        result = self.tool(20, "wait", {"timeout_seconds": 1})
        self.assertEqual(result["kind"], "wait_idle")
        self.assertFalse(result["acknowledged"])
        self.assertEqual(self.tool(21, "read")["deliveries"], [])
        self.assert_not_acknowledged()


class ServeRobustnessTests(unittest.TestCase):
    def test_unexpected_helper_output_still_answers_the_request(self):
        request = json.dumps({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                              "params": {"name": "codescribe_read", "arguments": {}}}) + "\n"
        output = io.StringIO()
        with patch.object(MCP.Mailbox, "call", side_effect=KeyError("read_delivery_ids")), \
                patch.object(sys, "stdin", io.StringIO(request)), contextlib.redirect_stdout(output):
            MCP.serve(DEMUX, Path(tempfile.gettempdir()), None)
        response = json.loads(output.getvalue())
        self.assertEqual(response["id"], 7)
        self.assertTrue(response["result"]["isError"])
        self.assertIn("read_delivery_ids", response["result"]["content"][0]["text"])


class ClaudeCodeMailboxTests(MailboxTests):
    """The whole native contract again, owned by a Claude Code session."""
    PROVIDER = "claude-code"


class ClaudeCodeProtocolTests(MCPProtocolTests):
    PROVIDER = "claude-code"


class ClaudeCodeWakeTests(MailboxFixture):
    PROVIDER = "claude-code"
    HOOK = SCRIPTS.parent / "plugins/codescribe/hooks/claude-code.json"

    def setUp(self):
        super().setUp()
        self.bind()
        self.environ = {"CLAUDE_CODE_SESSION_ID": self.session, "CLAUDE_PID": str(os.getpid())}

    def save(self):
        super().save()
        state = DEMUX.read_json(self.lease_path)
        state["pid"] = os.getpid()  # a live follower owns this lease
        DEMUX.atomic_json(self.lease_path, state)

    def bind(self, provider="claude-code"):
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings["7"] = {"audience": "fable", "provider": provider,
                             "provider_session_id": self.session, "bus": str(self.bus)}

    def wake(self, environ=None, window=0.6):
        errors = io.StringIO()
        with patch.object(sys, "stderr", errors):
            code = MCP.claude_code_wake(DEMUX, self.root, None, {"session_id": self.session},
                                        self.environ if environ is None else environ,
                                        window=window, interval=0.05)
        lines = [line for line in errors.getvalue().splitlines() if line]
        return code, (parse_wake_notice(lines[-1]) if lines else None)

    def test_existing_unread_wakes_once_then_only_for_new_deliveries(self):
        code, notice = self.wake()
        self.assertEqual(code, MCP.WAKE_EXIT)
        self.assertEqual(notice["kind"], "codescribe_mailbox_ready")
        self.assertEqual(notice["new_delivery_ids"], self.ids)
        self.assertEqual((notice["provider"], notice["channel"], notice["name"]), ("claude-code", "7", "fable"))
        code, notice = self.wake()
        self.assertEqual((code, notice["kind"]), (MCP.WAKE_EXIT, "codescribe_wait_idle"))
        self.pending.append(self.envelope("f" * 24))
        self.save()
        code, notice = self.wake()
        self.assertEqual(notice["new_delivery_ids"], ["f" * 24])
        self.assertEqual(notice["pending_count"], 6)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_arrival_during_silence_wakes_without_reading(self):
        self.pending = []
        self.save()
        outcome = queue.Queue()
        threading.Thread(target=lambda: outcome.put(self.wake(window=10)), daemon=True).start()
        time.sleep(0.3)
        self.assertTrue(outcome.empty())
        self.assertTrue(MCP.wake_armed(self.root, self.lease))
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        code, notice = outcome.get(timeout=5)
        self.assertEqual((code, notice["new_delivery_ids"]), (MCP.WAKE_EXIT, self.ids[:1]))
        self.assertFalse(MCP.wake_armed(self.root, self.lease))
        self.assert_not_acknowledged()

    def test_acknowledged_delivery_leaves_the_announced_set(self):
        self.wake()
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        self.wake()
        record = DEMUX.read_json(MCP.wake_paths(self.root, self.lease)[1])
        self.assertEqual(record["announced"], self.ids[1:])

    def test_only_claude_code_owner_of_an_attached_channel_waits(self):
        for environ in ({}, {"CLAUDE_CODE_SESSION_ID": "another-claude-session"}):
            with self.subTest(environ=environ):
                started = time.monotonic()
                self.assertEqual(self.wake(environ=environ, window=30), (0, None))
                self.assertLess(time.monotonic() - started, 1)
        self.bind(provider="claude")
        self.assertEqual(self.wake(window=30), (0, None))
        self.assertFalse(MCP.wake_paths(self.root, self.lease)[0].exists())

    def test_armed_lease_lets_a_later_stop_return_at_once(self):
        lock = MCP.wake_paths(self.root, self.lease)[0]
        lock.parent.mkdir(parents=True)
        with open(lock, "w") as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            self.assertTrue(MCP.wake_armed(self.root, self.lease))
            started = time.monotonic()
            self.assertEqual(self.wake(window=30), (0, None))
            self.assertLess(time.monotonic() - started, 1)

    def test_detach_or_host_exit_ends_the_wait_silently(self):
        self.pending = []
        self.save()
        outcome = queue.Queue()
        threading.Thread(target=lambda: outcome.put(self.wake(window=30)), daemon=True).start()
        time.sleep(0.3)
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings.pop("7")
        self.assertEqual(outcome.get(timeout=5), (0, None))
        self.bind()
        ended = subprocess.Popen(["true"])
        ended.wait()
        self.assertEqual(self.wake(environ=dict(self.environ, CLAUDE_PID=str(ended.pid)), window=30), (0, None))

    def test_lost_follower_is_announced_once_without_reading(self):
        self.pending = []
        super().save()  # no live follower pid
        code, notice = self.wake()
        self.assertEqual((code, notice["kind"], notice["problem"]),
                         (MCP.WAKE_EXIT, "codescribe_listener_problem", "follower_lost"))
        code, notice = self.wake()
        self.assertEqual(notice["kind"], "codescribe_wait_idle")
        self.assert_not_acknowledged()

    def hook(self, shell, command=None, environ=None):
        entry = json.loads(self.HOOK.read_text())["hooks"]["Stop"][0]["hooks"][0]
        script = (command or entry["command"]).replace('"$HOME/.local/bin/cs-bus"', f'"{SCRIPTS / "cs-bus"}"')
        env = {key: value for key, value in os.environ.items() if not key.startswith("CLAUDE_CODE")}
        env.update({"CODESCRIBE_AGENT_BRIDGE_HOME": str(self.root), "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"})
        env.update(self.environ if environ is None else environ)
        payload = json.dumps({"session_id": self.session, "hook_event_name": "Stop"})
        return subprocess.run([shell, "-c", script], input=payload, env=env,
                              capture_output=True, text=True, timeout=20)

    def test_plugin_hook_command_maps_only_the_wake_to_exit_two(self):
        for shell in ("/bin/bash", "/bin/zsh"):
            with self.subTest(shell=shell):
                (self.root / "runtime" / "native-wake").mkdir(parents=True, exist_ok=True)
                for path in (self.root / "runtime" / "native-wake").iterdir():
                    path.unlink()
                result = self.hook(shell)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("new message(s) for fable on channel 7", result.stderr)
                self.assertNotIn("{", result.stderr)
                junie = self.hook(shell, environ={})
                self.assertEqual((junie.returncode, junie.stderr), (0, ""))
                # A helper that cannot parse the hook exits 2 itself; that must
                # never become a model wake on every turn end, and never silence.
                entry = json.loads(self.HOOK.read_text())["hooks"]["Stop"][0]["hooks"][0]
                stale = self.hook(shell, command=entry["command"].replace("--claude-code-wake", "--no-such-wake"))
                self.assertEqual(stale.returncode, 1, stale.stderr)
                self.assertIn("unrecognized arguments", stale.stderr)
                self.assertIn("Codescribe: wake helper failed (exit 2)", stale.stderr)
        self.assert_not_acknowledged()

    def foreign_interpreter(self):
        """A host's interpreter variables: a PYTHONHOME whose stdlib matches no helper Python.

        Live 2026-10-15: a Vibecrafted-launched Claude Code inherited the
        launcher's 3.14 PYTHONHOME; the helper's `#!/usr/bin/env python3` died
        before any code ran and the hook mapped the crash to a silent exit 0.
        """
        home = Path(self.temp.name) / "foreign-python"
        (home / "lib/python3.14").mkdir(parents=True)
        return {"PYTHONHOME": str(home), "PYTHONPATH": str(home / "lib/python3.14")}

    def test_foreign_python_home_cannot_silence_the_wake(self):
        poison = self.foreign_interpreter()
        # The plugin's managed PATH, as the hook sees it; a pinned door earlier on
        # this test's own PATH could otherwise repair the interpreter for us.
        managed = f"{Path.home()}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
        env = {**os.environ, **self.environ, **poison, "CODESCRIBE_AGENT_BRIDGE_HOME": str(self.root), "PATH": managed}
        payload = json.dumps({"session_id": self.session, "hook_event_name": "Stop"})
        bare = subprocess.run([str(SCRIPTS / "cs-bus"), "--claude-code-wake"], input=payload, env=env,
                              capture_output=True, text=True, timeout=20)
        self.assertNotIn(bare.returncode, (0, MCP.WAKE_EXIT), "the fixture must poison a bare helper")
        entry = json.loads(self.HOOK.read_text())["hooks"]["Stop"][0]["hooks"][0]
        helper = re.search(r'env -u PYTHONHOME -u PYTHONPATH .*?--claude-code-wake', entry["command"])[0]
        for shell in ("/bin/bash", "/bin/zsh"):
            with self.subTest(shell=shell):
                for path in (self.root / "runtime" / "native-wake").glob("*"):
                    path.unlink()
                wake = self.hook(shell, command=helper, environ={**self.environ, **poison})
                self.assertEqual(wake.returncode, MCP.WAKE_EXIT, wake.stderr)
                for path in (self.root / "runtime" / "native-wake").glob("*"):
                    path.unlink()
                result = self.hook(shell, environ={**self.environ, **poison})
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("new message(s) for fable on channel 7", result.stderr)
        self.assertFalse((self.root / "runtime/hook-failures.log").exists())
        self.assert_not_acknowledged()

    def test_crashed_wake_helper_is_visible_and_leaves_a_receipt(self):
        crash = Path(self.temp.name) / "crashing cs-bus"
        crash.write_text("#!/bin/sh\necho 'Traceback (most recent call last):' >&2\n"
                         "echo \"ModuleNotFoundError: No module named '_struct'\" >&2\nexit 1\n")
        crash.chmod(0o755)
        hooks = json.loads(self.HOOK.read_text())["hooks"]
        for shell in ("/bin/bash", "/bin/zsh"):
            with self.subTest(shell=shell):
                log = self.root / "runtime/hook-failures.log"
                log.unlink(missing_ok=True)
                wake = self.hook(shell, command=hooks["Stop"][0]["hooks"][0]["command"].replace(
                    '"$HOME/.local/bin/cs-bus"', f'"{crash}"'))
                self.assertEqual(wake.returncode, 1, wake.stderr)
                self.assertIn("No module named '_struct'", wake.stderr)
                self.assertIn("Codescribe: wake helper failed (exit 1)", wake.stderr)
                probe = self.hook(shell, command=hooks["PostToolUse"][0]["hooks"][0]["command"].replace(
                    '"$HOME/.local/bin/cs-bus"', f'"{crash}"'))
                self.assertEqual((probe.returncode, probe.stdout), (0, ""))
                lines = log.read_text().splitlines()
                self.assertEqual(len(lines), 2, lines)
                self.assertRegex(lines[0], r"^\d{4}-\d\d-\d\dT\S+Z --claude-code-wake exit 1: "
                                           r"ModuleNotFoundError: No module named '_struct'$")
                self.assertIn("--claude-code-probe exit 1: ModuleNotFoundError", lines[1])
        self.assert_not_acknowledged()


class ClaudeCodeProbeTests(ClaudeCodeWakeTests):
    """PostToolUse probe: mid-turn surfacing with the same ledger as the Stop waiter."""

    def probe(self, environ=None):
        out = io.StringIO()
        with patch.object(sys, "stdout", out):
            code = MCP.claude_code_probe(DEMUX, self.root, None, {"session_id": self.session},
                                         self.environ if environ is None else environ)
        text = out.getvalue()
        if not text:
            return code, None
        context = json.loads(text)["hookSpecificOutput"]
        self.assertEqual(context["hookEventName"], "PostToolUse")
        return code, parse_wake_notice(context["additionalContext"])

    def test_probe_surfaces_each_unread_delivery_once_and_is_silent_afterwards(self):
        code, notice = self.probe()
        self.assertEqual(code, 0)
        self.assertEqual((notice["kind"], notice["new_delivery_ids"], notice["pending_count"]),
                         ("codescribe_mailbox_ready", self.ids, 5))
        self.assertEqual((notice["provider"], notice["channel"], notice["name"]), ("claude-code", "7", "fable"))
        self.assertEqual(self.probe(), (0, None), "An announced delivery is not surfaced after every tool call")
        self.pending.append(self.envelope("f" * 24))
        self.save()
        code, notice = self.probe()
        self.assertEqual((notice["new_delivery_ids"], notice["pending_count"]), (["f" * 24], 6))
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_probed_delivery_does_not_wake_the_turn_end_again(self):
        self.probe()
        code, notice = self.wake()
        self.assertEqual((code, notice["kind"]), (MCP.WAKE_EXIT, "codescribe_wait_idle"))
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        self.pending.append(self.envelope("e" * 24))
        self.save()
        code, notice = self.wake()
        self.assertEqual(notice["new_delivery_ids"], ["e" * 24])
        self.assertEqual(DEMUX.read_json(MCP.wake_paths(self.root, self.lease)[1])["announced"],
                         sorted(self.ids[1:] + ["e" * 24]))

    def test_probe_is_silent_for_other_sessions_unbound_channels_and_empty_mailboxes(self):
        for environ in ({}, {"CLAUDE_CODE_SESSION_ID": "another-claude-session"}):
            with self.subTest(environ=environ):
                self.assertEqual(self.probe(environ=environ), (0, None))
        self.pending = []
        self.save()
        self.assertEqual(self.probe(), (0, None))
        self.assertFalse(MCP.wake_paths(self.root, self.lease)[1].exists(), "Silence writes no ledger")
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings.pop("7")
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        self.assertEqual(self.probe(), (0, None))

    def test_plugin_post_tool_use_command_prints_context_only_for_a_new_delivery(self):
        entry = json.loads(self.HOOK.read_text())["hooks"]["PostToolUse"][0]["hooks"][0]
        for shell in ("/bin/bash", "/bin/zsh"):
            with self.subTest(shell=shell):
                (self.root / "runtime" / "native-wake").mkdir(parents=True, exist_ok=True)
                for path in (self.root / "runtime" / "native-wake").iterdir():
                    path.unlink()
                result = self.hook(shell, command=entry["command"])
                self.assertEqual(result.returncode, 0, result.stderr)
                context = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
                self.assertIn("new message(s) for fable on channel 7", context)
                again = self.hook(shell, command=entry["command"])
                self.assertEqual((again.returncode, again.stdout), (0, ""))
                junie = self.hook(shell, command=entry["command"], environ={})
                self.assertEqual((junie.returncode, junie.stdout), (0, ""))
        self.assert_not_acknowledged()


class PluginContractTests(unittest.TestCase):
    PLUGIN = SCRIPTS.parent / "plugins/codescribe"

    def load(self, relative):
        return json.loads((self.PLUGIN / relative).read_text())

    def test_claude_code_only_hooks_stay_out_of_the_shared_hook_file(self):
        manifest = self.load(".claude-plugin/plugin.json")
        self.assertEqual(manifest["hooks"], "./hooks/claude-code.json")
        shared = json.dumps(self.load("hooks/hooks.json"))
        self.assertNotIn("asyncRewake", shared)
        self.assertEqual(set(self.load("hooks/hooks.json")["hooks"]), {"SessionStart"})
        claude = self.load("hooks/claude-code.json")["hooks"]
        self.assertEqual(set(claude), {"SessionStart", "Stop", "PostToolUse"})
        for event in ("SessionStart", "Stop"):
            entry = claude[event][0]["hooks"][0]
            with self.subTest(event=event):
                self.assertTrue(entry["asyncRewake"])
                self.assertGreater(entry["timeout"], MCP.WAKE_SECONDS)
                self.assertIn(f'-eq {MCP.WAKE_EXIT} ]; then exit 2', entry["command"])
                self.assertNotIn("status=", entry["command"])  # read-only in zsh
        # The mid-turn probe is synchronous and can never wake or block a turn:
        # it ends with exit 0 whatever the helper returned.
        probe = claude["PostToolUse"][0]["hooks"][0]
        self.assertNotIn("asyncRewake", probe)
        self.assertLessEqual(probe["timeout"], 10)
        self.assertIn("--claude-code-probe", probe["command"])
        self.assertTrue(probe["command"].rstrip().endswith("exit 0"))

    def test_wait_windows_stay_below_measured_client_deadlines(self):
        self.assertLess(MCP.WAIT_SECONDS["junie"], 180)
        self.assertLess(MCP.WAIT_SECONDS["claude-code"], 1800)
        self.assertLessEqual(MCP.WAIT_SECONDS["omp"], MCP.WAIT_SECONDS["claude-code"])
        connect = next(tool for tool in MCP.TOOLS if tool["name"] == "codescribe_connect")
        self.assertEqual(connect["inputSchema"]["properties"]["provider"]["enum"], sorted(MCP.WAIT_SECONDS))
        self.assertEqual(MCP.NATIVE_WAKE_PROVIDERS, {"claude-code", "omp"})

    def test_omp_extension_runs_the_wake_from_the_managed_helper_path(self):
        # OMP loads `hooks/pre/*.ts` from a marketplace plugin as extension modules;
        # Claude Code and Junie read only the JSON hook files, so the TS module must
        # not appear there, and it must launch the same verified helper as .mcp.json.
        extension = (self.PLUGIN / "hooks/pre/codescribe-omp.ts").read_text()
        launcher = self.load(".mcp.json")["mcpServers"]["codescribe"]["args"][-1]
        self.assertIn('"$HOME/.local/bin/cs-bus"', launcher)
        self.assertIn('exec env -u PYTHONHOME -u PYTHONPATH "$HOME/.local/bin/cs-bus" --omp-wake --provider omp '
                      '--session "$1" --host-pid "$2"', extension)
        self.assertIn('export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"', extension)
        self.assertEqual(int(re.search(r"const WAKE_EXIT = (\d+)", extension)[1]), MCP.WAKE_EXIT)
        self.assertGreater(int(re.search(r"const WAKE_TIMEOUT_MS = ([\d_]+)", extension)[1].replace("_", "")),
                           MCP.WAKE_SECONDS * 1000)
        for relative in ("hooks/hooks.json", "hooks/claude-code.json", ".claude-plugin/plugin.json"):
            self.assertNotIn("codescribe-omp", json.dumps(self.load(relative)))


class OmpMailboxTests(MailboxTests):
    PROVIDER = "omp"


class OmpProtocolTests(MCPProtocolTests):
    PROVIDER = "omp"


class OmpWakeTests(MailboxFixture):
    PROVIDER = "omp"

    def setUp(self):
        super().setUp()
        self.bind()
        self.host = os.getpid()

    def save(self):
        super().save()
        state = DEMUX.read_json(self.lease_path)
        state["pid"] = os.getpid()  # a live follower owns this lease
        DEMUX.atomic_json(self.lease_path, state)

    def bind(self, provider="omp"):
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings["4"] = {"audience": "leon", "provider": provider,
                             "provider_session_id": self.session, "bus": str(self.bus)}

    def wake(self, session=None, host=None, window=0.6):
        errors = io.StringIO()
        with patch.object(sys, "stderr", errors):
            code = MCP.omp_wake(DEMUX, self.root, None, self.session if session is None else session,
                                self.host if host is None else host, window=window, interval=0.05)
        lines = [line for line in errors.getvalue().splitlines() if line]
        return code, (parse_wake_notice(lines[-1]) if lines else None)

    def test_existing_unread_wakes_once_then_only_for_new_deliveries(self):
        code, notice = self.wake()
        self.assertEqual(code, MCP.WAKE_EXIT)
        self.assertEqual(notice["kind"], "codescribe_mailbox_ready")
        self.assertEqual(notice["new_delivery_ids"], self.ids)
        self.assertEqual((notice["provider"], notice["channel"], notice["name"]), ("omp", "4", "leon"))
        code, notice = self.wake()
        self.assertEqual((code, notice["kind"]), (MCP.WAKE_EXIT, "codescribe_wait_idle"))
        self.pending.append(self.envelope("f" * 24))
        self.save()
        code, notice = self.wake()
        self.assertEqual(notice["new_delivery_ids"], ["f" * 24])
        self.assertEqual(notice["pending_count"], 6)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_arrival_during_silence_wakes_without_reading(self):
        self.pending = []
        self.save()
        outcome = queue.Queue()
        threading.Thread(target=lambda: outcome.put(self.wake(window=10)), daemon=True).start()
        time.sleep(0.3)
        self.assertTrue(outcome.empty())
        self.assertTrue(MCP.wake_armed(self.root, self.lease))
        self.assertTrue(self.mailbox.status()["native_wake_armed"])
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        code, notice = outcome.get(timeout=5)
        self.assertEqual((code, notice["new_delivery_ids"]), (MCP.WAKE_EXIT, self.ids[:1]))
        self.assertFalse(MCP.wake_armed(self.root, self.lease))
        self.assert_not_acknowledged()

    def test_only_the_bound_omp_session_waits(self):
        started = time.monotonic()
        self.assertEqual(self.wake(session="another-omp-session", window=30), (0, None))
        self.assertEqual(self.wake(session="", window=30), (0, None))
        self.assertEqual(self.wake(session="bad\nid", window=30), (0, None))
        self.assertLess(time.monotonic() - started, 1)
        self.bind(provider="claude-code")
        self.assertEqual(self.wake(window=30), (0, None))
        self.assertFalse(MCP.wake_paths(self.root, self.lease)[0].exists())

    def test_host_exit_ends_the_wait_silently(self):
        self.pending = []
        self.save()
        ended = subprocess.Popen(["true"])
        ended.wait()
        self.assertEqual(self.wake(host=ended.pid, window=30), (0, None))

    def test_cli_waiter_exits_wake_code_with_one_sentence(self):
        command = [sys.executable, str(SCRIPTS / "cs-bus"), "--omp-wake", "--provider", "omp",
                   "--session", self.session, "--host-pid", str(self.host), "--bridge-home", str(self.root)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, MCP.WAKE_EXIT, result.stderr)
        self.assertEqual(result.stdout, "")
        notice = parse_wake_notice(result.stderr.strip().splitlines()[-1])
        self.assertEqual((notice["kind"], notice["new_delivery_ids"]), ("codescribe_mailbox_ready", self.ids))
        self.assertIn("--provider omp", result.stderr)
        self.assertNotIn("{", result.stderr)
        for wrong in (["--omp-wake", "--session", self.session], ["--omp-wake", "--provider", "omp"],
                      ["--omp-wake", "--provider", "codex", "--session", self.session],
                      ["--claude-code-wake", "--provider", "omp", "--session", self.session]):
            with self.subTest(wrong=wrong):
                refused = subprocess.run([sys.executable, str(SCRIPTS / "cs-bus"), *wrong, "--bridge-home", str(self.root)],
                                         capture_output=True, text=True, timeout=20, input="{}")
                self.assertEqual(refused.returncode, 2, refused.stderr)


class SessionHookTests(unittest.TestCase):
    HOOK = SCRIPTS.parent / "plugins/codescribe/hooks/session.py"

    def run_hook(self, event, session="2bb934cc-f1d9-48df-a456-1f5f8190f8ae", exported=None, prompt=""):
        env = {key: value for key, value in os.environ.items() if key != "CLAUDE_CODE_SESSION_ID"}
        if exported is not None:
            env["CLAUDE_CODE_SESSION_ID"] = exported
        payload = json.dumps({"session_id": session, "hook_event_name": event, "prompt": prompt})
        return subprocess.run([sys.executable, str(self.HOOK)], input=payload, env=env,
                              capture_output=True, text=True, timeout=10)

    def context(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]

    def test_claude_code_identity_is_proven_by_its_exported_session(self):
        session = "2bb934cc-f1d9-48df-a456-1f5f8190f8ae"
        context = self.context(self.run_hook("SessionStart", exported=session))
        self.assertIn(f"provider=claude-code session_id={session}", context)
        self.assertIn("Stop hook", context)
        quiet = self.run_hook("UserPromptSubmit", exported=session)
        self.assertEqual((quiet.returncode, quiet.stdout), (0, ""))

    def test_junie_terminal_commands_are_not_changed_by_prompt_hook(self):
        # Junie strips ! before this hook, then executes additionalContext as
        # part of the Bash Task. The hook must be silent for every prompt.
        commands = ("ls -d .", "command -v sh", "printf '%s\\n' 'one <two>'",
                    "printf 'one\\n'\nprintf 'two\\n'")
        for exported in (None, "the-parent-claude-session"):
            for command in commands:
                with self.subTest(exported=exported, command=command):
                    hook = self.run_hook("UserPromptSubmit", session="session-261011-220514-1xgd",
                                         exported=exported, prompt=command)
                    self.assertEqual(hook.returncode, 0, hook.stderr)
                    output = json.loads(hook.stdout) if hook.stdout else {}
                    context = output.get("hookSpecificOutput", {}).get("additionalContext", "")
                    request = f"<additional_context>\n{context}\n</additional_context>\n\n{command}" if context else command
                    expected = subprocess.run(["/bin/bash", "-c", command], cwd=self.HOOK.parent,
                                              capture_output=True, text=True, timeout=10)
                    actual = subprocess.run(["/bin/bash", "-c", request], cwd=self.HOOK.parent,
                                            capture_output=True, text=True, timeout=10)
                    self.assertEqual((actual.returncode, actual.stdout, actual.stderr),
                                     (expected.returncode, expected.stdout, expected.stderr))
                    self.assertEqual(hook.stdout, "")

    def test_junie_session_start_does_not_emit_prompt_context(self):
        result = self.run_hook("SessionStart", session="session-261011-220514-1xgd")
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertNotIn("hookSpecificOutput", output)
        self.assertIn("provider=junie session_id=session-261011-220514-1xgd", output["systemMessage"])

    def test_junie_keeps_its_identity_even_inside_a_claude_code_shell(self):
        for exported in (None, "the-parent-claude-session"):
            with self.subTest(exported=exported):
                result = self.run_hook("SessionStart", session="session-261011-220514-1xgd", exported=exported)
                self.assertEqual(result.returncode, 0, result.stderr)
                output = json.loads(result.stdout)
                self.assertNotIn("hookSpecificOutput", output)
                self.assertIn("provider=junie session_id=session-261011-220514-1xgd", output["systemMessage"])

    def test_prompt_hook_is_silent_even_without_identity(self):
        result = self.run_hook("UserPromptSubmit", session="", prompt="ls")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))

    def test_missing_identity_is_refused(self):
        result = self.run_hook("SessionStart", session="")
        self.assertEqual(result.returncode, 1)
        self.assertIn("do not invent", result.stderr)

    def test_hook_command_survives_a_refusing_python_first_on_path(self):
        # Live Claude Code 2.1.296 run: a runtime-pinned python3 door first on
        # PATH refused the bare `python3`, so the model never got its identity.
        managed = [Path.home() / ".local/bin", Path("/opt/homebrew/bin"), Path("/usr/local/bin")]
        if not any((directory / "python3").is_file() for directory in managed):
            self.skipTest("no python3 in the plugin's managed interpreter directories")
        command = json.loads((self.HOOK.parent / "hooks.json").read_text())["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        session = "2bb934cc-f1d9-48df-a456-1f5f8190f8ae"
        with tempfile.TemporaryDirectory() as temporary:
            door = Path(temporary) / "python3"
            door.write_text("#!/bin/sh\necho 'no interpreter is pinned' >&2\nexit 1\n")
            door.chmod(0o755)
            env = {key: value for key, value in os.environ.items() if not key.startswith("CLAUDE_")}
            env.update({"PATH": f"{temporary}:/usr/bin:/bin", "CLAUDE_CODE_SESSION_ID": session,
                        "CLAUDE_PLUGIN_ROOT": str(self.HOOK.parents[1])})
            for shell in ("/bin/bash", "/bin/zsh"):
                with self.subTest(shell=shell):
                    result = subprocess.run([shell, "-c", command], env=env, capture_output=True, text=True, timeout=10,
                                            input=json.dumps({"session_id": session, "hook_event_name": "SessionStart"}))
                    self.assertIn(f"provider=claude-code session_id={session}", self.context(result))

    def test_hook_command_survives_a_foreign_python_home(self):
        # Live 2026-10-15: a Vibecrafted launcher leaked its 3.14 PYTHONHOME into
        # Claude Code; the managed python3 then died before reading the payload.
        managed = [Path("/opt/homebrew/bin"), Path("/usr/local/bin")]
        if not any((directory / "python3").is_file() for directory in managed):
            self.skipTest("no python3 in the plugin's managed interpreter directories")
        command = json.loads((self.HOOK.parent / "hooks.json").read_text())["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        session = "2bb934cc-f1d9-48df-a456-1f5f8190f8ae"
        with tempfile.TemporaryDirectory() as temporary:
            foreign = Path(temporary) / "foreign-python"
            (foreign / "lib/python3.14").mkdir(parents=True)
            env = {key: value for key, value in os.environ.items() if not key.startswith("CLAUDE_")}
            env.update({"PATH": "/usr/bin:/bin", "CLAUDE_CODE_SESSION_ID": session, "PYTHONHOME": str(foreign),
                        "PYTHONPATH": str(foreign / "lib/python3.14"), "CLAUDE_PLUGIN_ROOT": str(self.HOOK.parents[1])})
            for shell in ("/bin/bash", "/bin/zsh"):
                with self.subTest(shell=shell):
                    result = subprocess.run([shell, "-c", command], env=env, capture_output=True, text=True, timeout=10,
                                            input=json.dumps({"session_id": session, "hook_event_name": "SessionStart"}))
                    self.assertIn(f"provider=claude-code session_id={session}", self.context(result))


class NativeLaunchTests(unittest.TestCase):
    def test_every_plugin_command_drops_the_host_interpreter_variables(self):
        plugin = SCRIPTS.parent / "plugins/codescribe"
        hooks = json.loads((plugin / "hooks/claude-code.json").read_text())["hooks"]
        commands = [entry["command"] for event in hooks.values() for group in event for entry in group["hooks"]]
        commands += [entry["command"] for event in json.loads((plugin / "hooks/hooks.json").read_text())["hooks"].values()
                     for group in event for entry in group["hooks"]]
        commands.append(json.loads((plugin / ".mcp.json").read_text())["mcpServers"]["codescribe"]["args"][-1])
        commands.append((plugin / "hooks/pre/codescribe-omp.ts").read_text())
        for command in commands:
            with self.subTest(command=command[:60]):
                self.assertRegex(command, r'env -u PYTHONHOME -u PYTHONPATH (PATH="[^"]+" )?("\$HOME/\.local/bin/cs-bus"|python3)')

    def test_installed_mcp_launch_survives_a_foreign_python_home(self):
        config = json.loads((SCRIPTS.parent / "plugins/codescribe/.mcp.json").read_text())["mcpServers"]["codescribe"]
        with tempfile.TemporaryDirectory(prefix=".mcp-launch-", dir=Path(__file__).parent) as temporary:
            installed = Path(temporary) / "managed bin"
            installed.mkdir()
            for name in ("cs-bus", "bus-demux.py", "bus-mcp.py"):
                shutil.copy2(SCRIPTS / name, installed / name)
            foreign = Path(temporary) / "foreign-python"
            (foreign / "lib/python3.14").mkdir(parents=True)
            arguments = [value.replace('"$HOME/.local/bin/cs-bus"', f'"{installed / "cs-bus"}"')
                         for value in config["args"]]
            env = {**os.environ, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "PYTHONHOME": str(foreign),
                   "PYTHONPATH": str(foreign / "lib/python3.14")}
            request = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}) + "\n"
            result = subprocess.run([config["command"], *arguments], env=env, input=request,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            tools = json.loads(result.stdout)["result"]["tools"]
            self.assertTrue(any(tool["name"] == "codescribe_wait" for tool in tools))

    def test_installed_mcp_launch_survives_sparse_path_without_overriding_home(self):
        config = json.loads((SCRIPTS.parent / "plugins/codescribe/.mcp.json").read_text())["mcpServers"]["codescribe"]
        with tempfile.TemporaryDirectory(prefix=".mcp-launch-", dir=Path(__file__).parent) as temporary:
            installed = Path(temporary) / "managed bin"
            installed.mkdir()
            for name in ("cs-bus", "bus-demux.py", "bus-mcp.py"):
                shutil.copy2(SCRIPTS / name, installed / name)
            # Translate only the managed installation path into this fixture;
            # production helper bytes and the real HOME remain unchanged.
            arguments = [value.replace('"$HOME/.local/bin/cs-bus"', f'"{installed / "cs-bus"}"')
                         for value in config["args"]]
            env = {**os.environ, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"}
            request = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}) + "\n"
            result = subprocess.run([config["command"], *arguments], env=env, input=request,
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            tools = json.loads(result.stdout)["result"]["tools"]
            self.assertTrue(any(tool["name"] == "codescribe_wait" for tool in tools))


if __name__ == "__main__":
    unittest.main()
