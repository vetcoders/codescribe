"""Receipts: one file per fact under runtime/receipts/<message_id>/, one composer."""
import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_receipts", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

MESSAGE_ID = "0123456789abcdef01234567"
REACH = [{"name": "lena", "channel": "3"}, {"name": "igor", "channel": "7"}]


class ReceiptFixture(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()
        self.provider, self.session = "codex", "receipt-session"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        metadata = mock.patch.object(DEMUX, "NativeProviderModelReader", return_value=None)
        metadata.start()
        self.addCleanup(metadata.stop)

    def args(self, **overrides):
        values = dict(bridge_home=self.root, bus=self.bus, bus_overridden=True,
                      provider=self.provider, session=self.session, broadcast="quiet",
                      read_limit=8, read_bytes=65536, lease_ttl=120)
        values.update(overrides)
        return argparse.Namespace(**values)

    def lease(self, name="lena", session=None):
        lease = DEMUX.SessionLease(
            root=self.root, provider=self.provider, provider_session_id=session or self.session,
            name=name, bus=self.bus, requested_id=None, ttl_seconds=120,
            follow_from_end=False, coalesce=True)
        self.addCleanup(lease.close)
        return lease

    def bind(self, channel="3", name="lena", session=None):
        DEMUX.write_channel_binding(self.root, channel, name, self.provider, session or self.session, str(self.bus))

    def policy(self, value):
        row = {"schema": DEMUX.BROADCAST_POLICY_SCHEMA, "provider": self.provider,
               "provider_session_id": self.session, "lease_id": self.lease_id,
               "bus": str(self.bus), "policy": value, "emitted_at": DEMUX.utc_now()}
        DEMUX.atomic_json(DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus), row)

    def payload(self, lease, index=1, channel="0", **overrides):
        # A typed channel-0 copy as the follower sees it: one message id, the
        # publisher's frozen list, this copy's own recipient as delivery key.
        row = {"schema": DEMUX.EVENT_SCHEMA, "kind": "message", "source_event_id": f"{index:024x}",
               "producer_schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA,
               "message_id": MESSAGE_ID, "audience": "lena", "channel": channel, "origin_channel": "0",
               "text": "Iwo Iwo Iwo Iwo Iwo", "state_change_allowed": True,
               "recipients": [{"name": "lena", "channel": "3", "provider": self.provider,
                               "provider_session_id": self.session, "lease_id": self.lease_id,
                               "bus": str(self.bus)}],
               "target": {"mode": "broadcast", "channel": "0", "recipients": list(REACH)}}
        row.update(overrides)
        lease.enrich(row)
        return row

    def receipts(self):
        return DEMUX.receipts_directory(self.root, MESSAGE_ID)

    def cli(self, *arguments):
        output, error = io.StringIO(), io.StringIO()
        with mock.patch.object(sys, "argv", [str(HELPER), "--bridge-home", str(self.root),
                                             "--bus", str(self.bus), *arguments]), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            result = DEMUX.main()
        return result, output.getvalue(), error.getvalue()


class WriteOnceTests(ReceiptFixture):
    def test_a_receipt_is_written_once_and_the_first_content_stays(self):
        path = self.receipts() / "deadbeefdeadbeefdeadbeef.delivered.json"
        self.assertTrue(DEMUX.write_receipt_once(path, {"first": True}))
        self.assertFalse(DEMUX.write_receipt_once(path, {"first": False}))
        self.assertEqual(json.loads(path.read_text()), {"first": True})
        self.assertEqual([p.name for p in path.parent.iterdir()], [path.name])
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)

    def test_record_message_receipt_rewrites_nothing(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        first = DEMUX.record_message_receipt(self.root, payload, "delivered", name="lena", wake="on")
        content = first.read_text()
        again = DEMUX.record_message_receipt(self.root, payload, "delivered", name="lena", wake="quiet")
        self.assertEqual(again, first)
        self.assertEqual(first.read_text(), content)
        self.assertEqual(json.loads(content)["wake"], "on")

    def test_deliveries_without_a_message_id_leave_no_receipt(self):
        lease = self.lease()
        plain = {"schema": DEMUX.EVENT_SCHEMA, "kind": "seal", "source_event_id": "1" * 24,
                 "audience": "lena", "channel": "3", "text": "dictation", "utterance_id": "u1"}
        lease.enrich(plain)
        self.assertIsNone(DEMUX.record_message_receipt(self.root, plain, "delivered", name="lena"))
        self.assertFalse((self.root / "runtime" / "receipts").exists())


class FollowerReceiptTests(ReceiptFixture):
    def test_delivered_then_read_with_wake_from_the_policy_at_delivery(self):
        lease = self.lease()
        self.bind()
        self.policy("quiet")
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        delivered = self.receipts() / f"{payload['delivery_id']}.delivered.json"
        row = json.loads(delivered.read_text())
        self.assertEqual((row["kind"], row["name"], row["channel"], row["wake"]), ("delivered", "lena", "3", "quiet"))
        self.assertEqual((row["lease_id"], row["provider"], row["provider_session_id"]),
                         (self.lease_id, self.provider, self.session))
        self.assertEqual(row["sent_to"], REACH)
        self.assertNotIn("text", row)
        self.assertTrue(row["at"])
        # A restart re-offers the same copy: the first fact stays, no second file.
        self.assertFalse(lease.queue_delivery(dict(payload)))
        self.assertEqual(sorted(p.name for p in self.receipts().iterdir()), [delivered.name])
        with mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args(ack=[payload["delivery_id"]])), 0)
        read = self.receipts() / f"{payload['delivery_id']}.read.json"
        row = json.loads(read.read_text())
        self.assertEqual((row["kind"], row["name"], row["channel"]), ("read", "lena", "3"))
        self.assertNotIn("wake", row)
        self.assertTrue(row["read_at"])
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease_id / f"{payload['delivery_id']}.json")
        self.assertEqual(marker["message_id"], MESSAGE_ID)
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, payload["delivery_id"], payload))
        # Pre-receipt policy change does not rewrite history: the state is read, wake quiet.
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(state["sent_to"], REACH)
        self.assertEqual(len(state["recipients"]), 1)
        recipient = state["recipients"][0]
        self.assertEqual((recipient["name"], recipient["channel"], recipient["state"], recipient["wake"]),
                         ("lena", "3", "read", "quiet"))
        self.assertEqual(recipient["at"], row["read_at"])
        self.assertIsNone(state["spoken"])
        self.assertEqual(state["withdrawn"], [])

    def test_delivered_wake_on_under_the_default_policy(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        row = json.loads((self.receipts() / f"{payload['delivery_id']}.delivered.json").read_text())
        self.assertEqual(row["wake"], "on")

    def test_off_writes_skipped_instead_of_delivered_and_the_marker_names_the_message(self):
        lease = self.lease()
        self.bind()
        self.policy("off")
        payload = self.payload(lease)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(lease.queue_delivery(payload))
            self.assertFalse(lease.queue_delivery(dict(payload)))
        names = sorted(p.name for p in self.receipts().iterdir())
        self.assertEqual(names, [f"{payload['delivery_id']}.skipped.json"])
        row = json.loads((self.receipts() / names[0]).read_text())
        self.assertEqual((row["kind"], row["policy"], row["name"], row["channel"]), ("skipped", "off", "lena", "3"))
        self.assertEqual(row["sent_to"], REACH)
        marker = DEMUX.read_json(DEMUX.broadcast_skip_path(self.root, self.lease_id, payload["delivery_id"]))
        self.assertEqual(marker["message_id"], MESSAGE_ID)
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual([(r["state"], r["wake"]) for r in state["recipients"]], [("skipped", None)])

    def test_drafts_leave_no_receipt(self):
        lease = self.lease()
        self.bind()
        draft = self.payload(lease, kind="draft")
        self.assertTrue(lease.queue_delivery(draft))
        self.assertFalse(self.receipts().exists())

    def test_withdrawing_a_held_message_writes_withdrawn(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease, hold_until="2099-01-01T00:00:00+00:00")
        self.assertFalse(lease.queue_delivery(payload))
        self.assertIn(payload["delivery_id"], lease.held)
        self.assertFalse(self.receipts().exists())
        control = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "withdraw",
                   "provider": self.provider, "provider_session_id": self.session,
                   "lease_id": self.lease_id, "bus": str(self.bus), "identity": MESSAGE_ID,
                   "control_id": "c" * 24, "emitted_at": "2026-10-26T06:00:00+00:00"}
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(lease.control_delivery(control), [])
        path = self.receipts() / f"withdrawn.{payload['delivery_id']}.json"
        row = json.loads(path.read_text())
        self.assertEqual((row["kind"], row["control_id"], row["at"]),
                         ("withdrawn", "c" * 24, "2026-10-26T06:00:00+00:00"))
        self.assertEqual(sorted(p.name for p in self.receipts().iterdir()), [path.name])
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(state["recipients"], [])
        self.assertEqual(state["withdrawn"], [{"delivery_id": payload["delivery_id"], "control_id": "c" * 24,
                                               "at": "2026-10-26T06:00:00+00:00"}])

    def test_a_handover_reads_the_delivery_made_to_the_old_lease_as_one_recipient(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        # The same delivery id read under the next session's lease (the
        # mailbox was inherited) is the same recipient, not a second one.
        successor = dict(payload, lease_id="f" * 32, provider_session_id="next-session")
        DEMUX.record_message_receipt(self.root, successor, "read", name="lena")
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(len(state["recipients"]), 1)
        self.assertEqual((state["recipients"][0]["state"], state["recipients"][0]["lease_id"]), ("read", "f" * 32))


class PlaybackReceiptTests(ReceiptFixture):
    def control(self, ticket):
        reply = {"reply_id": MESSAGE_ID, "provider": self.provider, "provider_session_id": self.session,
                 "lease_id": self.lease_id, "text": "never read here"}
        mutes = {"global_muted": False, "agent_muted": False, "effective_muted": False, "mute_scope": None}
        patches = [mock.patch.object(DEMUX, "playback_mute_snapshot", return_value=dict(mutes)),
                   mock.patch.object(DEMUX, "publish_reply_event", return_value=None),
                   mock.patch.object(DEMUX, "emit")]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        control = DEMUX.ReplyPlaybackControl(self.args(), self.bus, reply, ticket)
        self.addCleanup(control.close)
        return control

    def test_the_first_terminal_state_of_a_ticket_is_the_one_file(self):
        ticket = "a" * 24
        control = self.control(ticket)
        control.publish("waiting")
        control.publish("playing")
        self.assertFalse(self.receipts().exists())
        control.publish("refused", reason="take_live")
        control.publish("failed", error="speech failed (RuntimeError)", reason="speech_exception")
        path = self.receipts() / f"playback.{ticket}.json"
        row = json.loads(path.read_text())
        self.assertEqual((row["kind"], row["state"], row["reason"], row["spoken"]),
                         ("playback", "refused", "take_live", False))
        self.assertEqual((row["provider"], row["provider_session_id"]), (self.provider, self.session))
        self.assertNotIn("text", row)
        self.assertNotIn("tts_error", row)
        self.assertEqual(sorted(p.name for p in self.receipts().iterdir()), [path.name])

    def test_spoken_wins_over_a_later_refusal_in_the_composed_state(self):
        first = self.control("b" * 24)
        first.publish("spoken")
        second = self.control("c" * 24)
        second.publish("refused", reason="muted")
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual((state["spoken"]["state"], state["spoken"]["spoken"], state["spoken"]["playback_ticket"]),
                         ("spoken", True, "b" * 24))

    def test_without_a_completed_playback_the_latest_terminal_one_is_reported(self):
        first = self.control("d" * 24)
        first.publish("refused", reason="take_live")
        second = self.control("e" * 24)
        with mock.patch.object(DEMUX, "utc_now", return_value="2099-01-01T00:00:00+00:00"):
            second.publish("failed", error="x", reason="speech_exception")
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual((state["spoken"]["state"], state["spoken"]["reason"], state["spoken"]["spoken"]),
                         ("failed", "speech_exception", False))


class ReceiptStateTests(ReceiptFixture):
    def write(self, name, row):
        DEMUX.write_receipt_once(self.receipts() / name, {"schema": DEMUX.MESSAGE_RECEIPT_SCHEMA,
                                                          "message_id": MESSAGE_ID, **row})

    def test_the_farthest_file_per_delivery_wins_and_foreign_files_are_ignored(self):
        d1, d2, d3 = "1" * 24, "2" * 24, "3" * 24
        self.write(f"{d1}.delivered.json", {"kind": "delivered", "delivery_id": d1, "name": "lena", "channel": "3",
                                            "lease_id": "l1", "wake": "on", "at": "2026-10-26T00:34:00+00:00",
                                            "sent_to": REACH})
        self.write(f"{d1}.read.json", {"kind": "read", "delivery_id": d1, "name": "lena", "channel": "3",
                                       "lease_id": "l1", "read_at": "2026-10-26T05:04:00+00:00"})
        self.write(f"{d2}.delivered.json", {"kind": "delivered", "delivery_id": d2, "name": "igor", "channel": "7",
                                            "lease_id": "l2", "wake": "quiet", "at": "2026-10-26T00:34:01+00:00"})
        self.write(f"{d3}.skipped.json", {"kind": "skipped", "delivery_id": d3, "name": "nina", "channel": "2",
                                          "lease_id": "l3", "policy": "off", "at": "2026-10-26T00:34:02+00:00"})
        # A file whose name and content disagree, a foreign schema and a
        # foreign message id are not facts about this message.
        self.write(f"{d2}.read.json", {"kind": "read", "delivery_id": d3, "name": "x", "channel": "9"})
        (self.receipts() / "notes.json").write_text(json.dumps({"schema": "other", "message_id": MESSAGE_ID}))
        self.write("playback.ffffffffffffffffffffffff.json", {"kind": "playback", "message_id": "f" * 24,
                                                               "playback_ticket": "f" * 24, "state": "spoken"})
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(state["message_id"], MESSAGE_ID)
        self.assertEqual(state["sent_to"], REACH)
        self.assertEqual([(r["name"], r["channel"], r["state"], r["wake"], r["at"]) for r in state["recipients"]], [
            ("igor", "7", "delivered", "quiet", "2026-10-26T00:34:01+00:00"),
            ("nina", "2", "skipped", None, "2026-10-26T00:34:02+00:00"),
            ("lena", "3", "read", "on", "2026-10-26T05:04:00+00:00"),
        ])
        self.assertEqual(state["recipients"][2]["delivered_at"], "2026-10-26T00:34:00+00:00")
        self.assertIsNone(state["spoken"])
        self.assertEqual(state["withdrawn"], [])

    def test_a_message_without_receipts_has_an_empty_state(self):
        self.assertEqual(DEMUX.receipt_state(self.root, MESSAGE_ID),
                         {"message_id": MESSAGE_ID, "sent_to": None, "recipients": [],
                          "withdrawn": [], "spoken": None})


class ReceiptsCommandTests(ReceiptFixture):
    def lease_file(self, name="lena", pending=()):
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "name": name, "bus": str(self.bus),
            "pending": list(pending), "held": {}})

    def test_a_copy_holder_gets_one_json_line_without_the_text(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        lease.close()
        code, output, error = self.cli("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", self.session)
        self.assertEqual((code, error), (0, ""))
        lines = output.splitlines()
        self.assertEqual(len(lines), 1)
        row = json.loads(lines[0])
        self.assertEqual(row["kind"], "receipts")
        self.assertEqual(row["sent_to"], REACH)
        self.assertEqual([(r["name"], r["state"], r["wake"]) for r in row["recipients"]], [("lena", "delivered", "on")])
        self.assertNotIn("Iwo", output)
        self.assertNotIn("text", row)

    def test_a_stranger_is_refused_without_learning_whether_the_message_exists(self):
        self.lease_file()
        DEMUX.write_receipt_once(self.receipts() / f"{'1' * 24}.delivered.json", {
            "schema": DEMUX.MESSAGE_RECEIPT_SCHEMA, "kind": "delivered", "message_id": MESSAGE_ID,
            "delivery_id": "1" * 24, "name": "igor", "channel": "7", "lease_id": "l2", "wake": "on", "at": "t"})
        for identity in (MESSAGE_ID, "9" * 24):
            with self.subTest(identity=identity):
                code, output, error = self.cli("--receipts", identity, "--provider", self.provider, "--session", self.session)
                self.assertEqual((code, error), (3, ""))
                row = json.loads(output)
                self.assertEqual((row["kind"], row["message_id"]), ("receipts_refused", identity))
                self.assertEqual(set(row), {"kind", "message_id", "reason"})

    def test_the_author_reads_receipts_of_its_own_message(self):
        DEMUX.atomic_json(self.root / "runtime" / "reply-sources" / f"{MESSAGE_ID}.json", {
            "schema": DEMUX.REPLY_SOURCE_SCHEMA, "reply_id": MESSAGE_ID, "provider": self.provider,
            "provider_session_id": self.session, "lease_id": self.lease_id, "bus": str(self.bus),
            "source": {"offset": 0, "length": 1}, "copies": []})
        DEMUX.write_receipt_once(self.receipts() / "playback.aaaaaaaaaaaaaaaaaaaaaaaa.json", {
            "schema": DEMUX.MESSAGE_RECEIPT_SCHEMA, "kind": "playback", "message_id": MESSAGE_ID,
            "playback_ticket": "a" * 24, "state": "spoken", "reason": None, "spoken": True, "at": "t"})
        code, output, _ = self.cli("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", self.session)
        self.assertEqual(code, 0)
        row = json.loads(output)
        self.assertEqual((row["spoken"]["state"], row["spoken"]["spoken"]), ("spoken", True))
        self.assertEqual(row["recipients"], [])
        # Another session is not the author.
        code, output, _ = self.cli("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", "other")
        self.assertEqual((code, json.loads(output)["kind"]), (3, "receipts_refused"))

    def test_a_name_on_sent_to_or_a_pending_copy_counts_as_holding_a_copy(self):
        self.bind(channel="7", name="igor")
        self.lease_file(name="igor")
        DEMUX.write_receipt_once(self.receipts() / f"{'1' * 24}.delivered.json", {
            "schema": DEMUX.MESSAGE_RECEIPT_SCHEMA, "kind": "delivered", "message_id": MESSAGE_ID,
            "delivery_id": "1" * 24, "name": "lena", "channel": "3", "lease_id": "l1", "wake": "on", "at": "t",
            "sent_to": REACH})
        code, output, _ = self.cli("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", self.session)
        self.assertEqual((code, json.loads(output)["kind"]), (0, "receipts"))
        # A pending copy in this mailbox, before any receipt file exists for it.
        other = "5" * 24
        self.lease_file(name="igor", pending=[{"delivery_id": "6" * 24, "message_id": other,
                                               "lease_id": self.lease_id, "kind": "message"}])
        code, output, _ = self.cli("--receipts", other, "--provider", self.provider, "--session", self.session)
        self.assertEqual((code, json.loads(output)["kind"]), (0, "receipts"))
        self.assertEqual(json.loads(output)["recipients"], [])

    def test_receipts_needs_an_owner_and_travels_alone(self):
        for arguments in (("--receipts", MESSAGE_ID),
                          ("--receipts", "not-hex", "--provider", self.provider, "--session", self.session),
                          ("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", self.session,
                           "--ack", "1" * 24)):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as raised:
                self.cli(*arguments)
            self.assertEqual(raised.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
