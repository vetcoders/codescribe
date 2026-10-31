"""Receipts: one file per fact under runtime/receipts/<message_id>/, one composer."""
import argparse
import contextlib
import hashlib
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
        self.assertIsNone(again)
        self.assertEqual(first.read_text(), content)
        self.assertEqual(json.loads(content)["wake"], "on")

    def test_wake_is_on_quiet_or_the_backfill_null(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        with self.assertRaises(ValueError):
            DEMUX.record_message_receipt(self.root, payload, "delivered", name="lena", wake="loud")
        path = DEMUX.record_message_receipt(self.root, payload, "delivered", name="lena", wake=None,
                                            at="2026-10-20T09:00:00Z")
        row = json.loads(path.read_text())
        self.assertIsNone(row["wake"])
        self.assertEqual(row["at"], "2026-10-20T09:00:00Z")

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
        # igor is on the list and left no file: counted, listed as no_receipt.
        self.assertEqual([(r["name"], r["state"]) for r in state["recipients"]],
                         [("lena", "read"), ("igor", "no_receipt")])
        self.assertEqual(state["denominator"], 2)
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
        self.assertEqual([(r["name"], r["state"], r["wake"]) for r in state["recipients"]],
                         [("lena", "skipped", None), ("igor", "no_receipt", None)])
        # lena is on the frozen list, but her copy was skipped at delivery:
        # the skip wins, she stays listed as skipped and leaves the count.
        self.assertEqual(state["denominator"], 1)

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
        self.assertEqual([(r["name"], r["channel"], r["state"]) for r in state["recipients"]],
                         [("lena", "3", "read"), ("igor", "7", "no_receipt")])
        self.assertEqual(state["denominator"], 2)
        read = json.loads((self.receipts() / f"{payload['delivery_id']}.read.json").read_text())
        self.assertEqual((read["lease_id"], read["provider_session_id"]), ("f" * 32, "next-session"))

    def test_a_crash_between_read_and_the_ack_marker_is_completed_by_the_next_ack(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        marker = self.root / "acknowledgments" / self.lease_id / f"{payload['delivery_id']}.json"
        read = self.receipts() / f"{payload['delivery_id']}.read.json"
        with mock.patch.object(DEMUX, "atomic_json", side_effect=OSError("power lost")), \
                self.assertRaises(OSError), mock.patch.object(DEMUX, "emit"):
            DEMUX.acknowledge_delivery(self.args(ack=[payload["delivery_id"]]))
        self.assertTrue(read.exists(), "the retryable half is written first")
        self.assertFalse(marker.exists())
        content = read.read_text()
        with mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args(ack=[payload["delivery_id"]])), 0)
        self.assertTrue(marker.exists())
        self.assertEqual(read.read_text(), content)
        self.assertEqual(DEMUX.receipt_state(self.root, MESSAGE_ID)["recipients"][0]["state"], "read")

    def test_the_projection_names_agents_by_name_and_channel_only(self):
        lease = self.lease()
        self.bind()
        payload = self.payload(lease)
        self.assertTrue(lease.queue_delivery(payload))
        DEMUX.write_receipt_once(self.receipts() / "playback.aaaaaaaaaaaaaaaaaaaaaaaa.json", {
            "schema": DEMUX.MESSAGE_RECEIPT_SCHEMA, "kind": "playback", "message_id": MESSAGE_ID,
            "playback_ticket": "a" * 24, "state": "spoken", "reason": None, "spoken": True,
            "provider": "claude-code", "provider_session_id": "author-session", "lease_id": "e" * 32, "at": "t"})
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(set(state["recipients"][0]),
                         {"delivery_id", "name", "channel", "state", "wake", "delivered_at", "at"})
        self.assertEqual(set(state["spoken"]), {"state", "reason", "spoken", "at", "playback_ticket"})
        self.assertNotIn(self.session, json.dumps(state))
        self.assertNotIn("author-session", json.dumps(state))
        self.assertNotIn(self.lease_id, json.dumps(state))


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
                         {"message_id": MESSAGE_ID, "sent_to": None, "recipients": [], "denominator": 0,
                          "withdrawn": [], "spoken": None})

    def test_sent_json_owns_sent_to_and_members_without_a_file_are_no_receipt(self):
        d1, d2, d4 = "1" * 24, "2" * 24, "4" * 24
        three = [*REACH, {"name": "nina", "channel": "2"}]
        self.write("sent.json", {"kind": "sent", "sent_to": three, "bus": None, "at": "2026-10-26T00:34:00Z"})
        # A recipient file carrying an older, shorter stamp does not override the publisher's.
        self.write(f"{d1}.delivered.json", {"kind": "delivered", "delivery_id": d1, "name": "lena", "channel": "3",
                                            "wake": "on", "at": "2026-10-26T00:34:01Z", "sent_to": REACH})
        self.write(f"{d1}.read.json", {"kind": "read", "delivery_id": d1, "name": "lena", "channel": "3",
                                       "read_at": "2026-10-26T05:04:00Z"})
        self.write(f"{d2}.delivered.json", {"kind": "delivered", "delivery_id": d2, "name": "igor", "channel": "7",
                                            "wake": "quiet", "at": "2026-10-26T00:34:02Z"})
        self.write(f"{d4}.skipped.json", {"kind": "skipped", "delivery_id": d4, "name": "kuba", "channel": "5",
                                          "policy": "off", "at": "2026-10-26T00:34:03Z"})
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(state["sent_to"], three)
        self.assertEqual([(r["name"], r["state"]) for r in state["recipients"]],
                         [("igor", "delivered"), ("kuba", "skipped"), ("lena", "read"), ("nina", "no_receipt")])
        self.assertEqual(state["recipients"][3], {"delivery_id": None, "name": "nina", "channel": "2",
                                                  "state": "no_receipt", "wake": None, "delivered_at": None, "at": None})
        # Three on the list (nina counted although unreached); kuba skipped is shown, not counted.
        self.assertEqual(state["denominator"], 3)

    def test_without_sent_json_the_denominator_comes_from_the_recipients_files(self):
        d1, d2, d3 = "1" * 24, "2" * 24, "3" * 24
        self.write(f"{d1}.delivered.json", {"kind": "delivered", "delivery_id": d1, "name": "lena", "channel": "3",
                                            "wake": "on", "at": "t1", "sent_to": REACH})
        self.write(f"{d2}.read.json", {"kind": "read", "delivery_id": d2, "name": "igor", "channel": "7", "read_at": "t2"})
        # An agent delivered to outside the stamped list (an app-stamped take) still counts.
        self.write(f"{d3}.delivered.json", {"kind": "delivered", "delivery_id": d3, "name": "kuba", "channel": "5",
                                            "wake": "on", "at": "t3"})
        state = DEMUX.receipt_state(self.root, MESSAGE_ID)
        self.assertEqual(state["sent_to"], REACH)
        self.assertEqual(state["denominator"], 3)
        self.assertNotIn("no_receipt", {r["state"] for r in state["recipients"]})

    def test_the_fixture_directories_pin_the_exact_state(self):
        fixtures = Path(__file__).resolve().parent / "fixtures" / "receipts"
        cases = sorted(p for p in fixtures.iterdir() if p.is_dir())
        self.assertGreaterEqual(len(cases), 7)
        self.assertLessEqual({"skipped-on-sent-to", "delivered-unread-unknown-wake"}, {case.name for case in cases})
        for case in cases:
            with self.subTest(case=case.name):
                expected = json.loads((case / "expected.json").read_text())
                self.assertEqual(DEMUX.receipt_state(case, expected["message_id"]), expected)
                for path in case.rglob("*.json"):
                    self.assertNotIn("text", json.loads(path.read_text()), path.name)


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
        self.assertEqual([(r["name"], r["state"], r["wake"]) for r in row["recipients"]],
                         [("lena", "delivered", "on"), ("igor", "no_receipt", None)])
        self.assertEqual(row["denominator"], 2)
        self.assertNotIn("Iwo", output)
        self.assertNotIn("text", row)

        def keys(value):
            if isinstance(value, dict):
                return set(value) | {k for v in value.values() for k in keys(v)}
            if isinstance(value, list):
                return {k for v in value for k in keys(v)}
            return set()
        self.assertFalse(keys(row) & {"provider", "provider_session_id", "lease_id", "bus"})

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

    def test_both_channel_zero_publishers_write_sent_json_once(self):
        for channel, name in (("2", "nina"), ("7", "igor")):
            bus = self.home / f"bus-{channel}.jsonl"
            lid = DEMUX.lease_identifier("codex", name)
            DEMUX.write_channel_binding(self.root, channel, name, "codex", name, str(bus))
            DEMUX.atomic_json(self.root / "leases" / f"{lid}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "provider": "codex", "provider_session_id": name,
                "lease_id": lid, "name": name, "bus": str(bus)})
        receipt = {"stream_id": "fixture", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}
        with mock.patch.object(DEMUX, "live_follower_pid", return_value=123), \
                mock.patch.object(DEMUX, "publish_reply_event", return_value=receipt) as publish, \
                mock.patch.object(DEMUX, "emit"), \
                mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"hello"))):
            self.assertEqual(DEMUX.send_text_command(argparse.Namespace(bridge_home=self.root, channel="0")), 0)
        typed = publish.call_args_list[0].args[1]["message_id"]
        path = DEMUX.receipts_directory(self.root, typed) / "sent.json"
        row = json.loads(path.read_text())
        self.assertEqual((row["kind"], row["message_id"], row["bus"]), ("sent", typed, None))
        self.assertEqual(row["sent_to"], [{"name": "nina", "channel": "2"}, {"name": "igor", "channel": "7"}])
        self.assertEqual(set(row), {"schema", "kind", "message_id", "sent_to", "bus", "at"})
        self.assertFalse(DEMUX.record_sent_receipt(self.root, typed, [], bus=None, at="later"))
        self.assertEqual(json.loads(path.read_text()), row)
        self.assertEqual(DEMUX.receipt_state(self.root, typed)["denominator"], 2)
        # A peer broadcast from nina (channel 2) reaches igor only, and says so.
        with mock.patch.object(DEMUX, "live_follower_pid", return_value=None), \
                mock.patch.object(DEMUX, "publish_reply_event", return_value=receipt) as publish, \
                mock.patch.object(DEMUX, "remember_published_reply"), mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.send_peer_command(argparse.Namespace(
                bridge_home=self.root, provider="codex", session="nina", send="fala", to="0")), 0)
        peer = publish.call_args_list[0].args[1]["message_id"]
        row = json.loads((DEMUX.receipts_directory(self.root, peer) / "sent.json").read_text())
        self.assertEqual(row["sent_to"], [{"name": "igor", "channel": "7"}])
        self.assertEqual(row["bus"], str(self.home / "bus-2.jsonl"))
        state = DEMUX.receipt_state(self.root, peer)
        self.assertEqual(([(r["name"], r["state"]) for r in state["recipients"]], state["denominator"]),
                         ([("igor", "no_receipt")], 1))

    def test_receipts_needs_an_owner_and_travels_alone(self):
        for arguments in (("--receipts", MESSAGE_ID),
                          ("--receipts", "not-hex", "--provider", self.provider, "--session", self.session),
                          ("--receipts", MESSAGE_ID, "--provider", self.provider, "--session", self.session,
                           "--ack", "1" * 24)):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as raised:
                self.cli(*arguments)
            self.assertEqual(raised.exception.code, 2)


class BackfillTests(ReceiptFixture):
    """`--backfill-receipts`: yesterday's markers get today's files, once."""

    M1, M2, M3 = "0123456789abcdef00000101", "0123456789abcdef00000102", "0123456789abcdef00000103"
    TICKET, PLAYING = "f" * 24, "e" * 24
    IGOR_SESSION = "igor-session"

    def lease_state(self, lease_id, session, name):
        DEMUX.atomic_json(self.root / "leases" / f"{lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": lease_id, "provider": self.provider,
            "provider_session_id": session, "name": name, "bus": str(self.bus), "pending": [], "held": {}})

    def history(self):
        """A bridge home as a helper older than receipts left it."""
        lease = self.lease()
        self.bind()
        self.bind(channel="7", name="igor", session=self.IGOR_SESSION)
        igor = DEMUX.lease_identifier(self.provider, self.IGOR_SESSION)
        self.lease_state(self.lease_id, self.session, "lena")
        self.lease_state(igor, self.IGOR_SESSION, "igor")
        # Lena's journal: two deliveries (one stamped with the sent list, one
        # not), a draft in between. One is acknowledged, one is unread.
        read = self.payload(lease, 1, message_id=self.M1, emitted_at="2026-10-20T09:00:00Z")
        unread = self.payload(lease, 2, message_id=self.M2, emitted_at="2026-10-20T09:00:02Z", target=None)
        draft = self.payload(lease, 3, kind="draft", message_id=self.M2, emitted_at="2026-10-20T09:00:01Z")
        _, journal = DEMUX.follower_paths(self.root, self.lease_id)
        journal.parent.mkdir(parents=True, exist_ok=True)
        for row in (read, draft, unread):
            DEMUX.emit_follower(row, journal, None, human_output=False)
        # An ACK marker as the old helper wrote it: message_id only inside the envelope.
        markers = self.root / "acknowledgments" / self.lease_id
        DEMUX.atomic_json(markers / f"{read['delivery_id']}.json", {
            "lease_id": self.lease_id, "delivery_id": read["delivery_id"], "provider": self.provider,
            "provider_session_id": self.session, "read_at": "2026-10-20T10:00:00Z", "bus": str(self.bus),
            "envelope": DEMUX.receipt_envelope(read, str(self.bus))})
        # A marker of this generation (message_id on top) whose journal is gone.
        gone = self.payload(lease, 4, message_id=self.M3, target=None)
        DEMUX.atomic_json(markers / f"{gone['delivery_id']}.json", {
            "lease_id": self.lease_id, "delivery_id": gone["delivery_id"], "provider": self.provider,
            "provider_session_id": self.session, "read_at": "2026-10-20T10:01:00Z", "bus": str(self.bus),
            "message_id": self.M3, "envelope": DEMUX.receipt_envelope(gone, str(self.bus))})
        # The oldest marker shape: no envelope at all, nothing to tie it to.
        DEMUX.atomic_json(markers / f"{'9' * 24}.json", {"lease_id": self.lease_id, "delivery_id": "9" * 24})
        # Igor's copy of the first message was skipped (channel zero off).
        DEMUX.atomic_json(DEMUX.broadcast_skip_path(self.root, igor, "4" * 24), {
            "schema": DEMUX.BROADCAST_POLICY_SCHEMA, "kind": "broadcast_skipped", "delivery_id": "4" * 24,
            "lease_id": igor, "provider": self.provider, "provider_session_id": self.IGOR_SESSION,
            "bus": str(self.bus), "message_id": self.M1, "origin_channel": "0", "policy": "off",
            "emitted_at": "2026-10-20T09:00:05Z"})
        # The first message was heard; the second is still playing.
        playback = self.root / "runtime" / "reply-playback"
        playback.mkdir(parents=True, exist_ok=True)
        for reply_id, ticket, state in ((self.M1, self.TICKET, "spoken"), (self.M2, self.PLAYING, "playing")):
            DEMUX.atomic_json(playback / f"{reply_id}.{ticket}.json", {
                "schema": DEMUX.REPLY_CONTROL_SCHEMA, "kind": "agent_reply_playback",
                "emitted_at": "2026-10-20T10:05:00Z", "reply_id": reply_id, "provider": self.provider,
                "provider_session_id": self.session, "lease_id": self.lease_id, "playback_ticket": ticket,
                "state": state, "spoken": state == "spoken", "bus": str(self.bus)})
            (playback / f"{reply_id}.{ticket}.lock").write_bytes(b"")
        (playback / f"{self.M2}.{self.PLAYING}.stop.json").write_text("{}")
        return {"read": read, "unread": unread, "gone": gone, "igor": igor}

    def sources(self):
        digests = {}
        for directory in ("acknowledgments", "broadcast-skipped", "leases", "runtime/followers",
                          "runtime/reply-playback"):
            for path in sorted((self.root / directory).rglob("*")):
                if path.is_file():
                    digests[str(path.relative_to(self.root))] = hashlib.sha256(path.read_bytes()).hexdigest()
        return digests

    def backfill(self, *arguments):
        output, error = io.StringIO(), io.StringIO()
        with mock.patch.object(sys, "argv", [str(HELPER), "--bridge-home", str(self.root), *arguments]), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            result = DEMUX.main()
        return result, output.getvalue(), error.getvalue()

    EXPECTED = {"kind": "receipts_backfill", "dry_run": False, "existing": 0, "unmapped": 1, "mapped_by_bus": 0,
                "failed": 0,
                "written": {"delivered": 2, "read": 2, "skipped": 1, "playback": 1, "sent": 1}}

    def test_a_pending_copy_is_delivered_too_and_the_journal_file_wins(self):
        lease = self.lease()
        self.bind()
        quiet = self.payload(lease, 5, message_id="0123456789abcdef00000104", emitted_at="2026-10-20T11:00:00Z",
                             target=None)
        both = self.payload(lease, 1, message_id=self.M1, emitted_at="2026-10-20T09:00:00Z")
        draft = self.payload(lease, 6, kind="draft", message_id=self.M1, emitted_at="2026-10-20T09:00:01Z")
        untied = self.payload(lease, 7, message_id="not-a-message", emitted_at="2026-10-20T09:00:02Z", target=None)
        _, journal = DEMUX.follower_paths(self.root, self.lease_id)
        journal.parent.mkdir(parents=True, exist_ok=True)
        DEMUX.emit_follower(both, journal, None, human_output=False)
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "name": "lena", "bus": str(self.bus),
            "pending": [quiet, both, draft, untied], "held": {}})
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual((result, error), (0, ""))
        self.assertEqual(json.loads(output), {**self.EXPECTED, "unmapped": 1, "written": {
            "delivered": 2, "read": 0, "skipped": 0, "playback": 0, "sent": 1}})
        row = json.loads((DEMUX.receipts_directory(self.root, "0123456789abcdef00000104")
                          / f"{quiet['delivery_id']}.delivered.json").read_text())
        self.assertEqual((row["wake"], row["at"], row["name"], row["channel"]), (None, "2026-10-20T11:00:00Z", "lena", "3"))
        self.assertNotIn("text", row)
        state = DEMUX.receipt_state(self.root, "0123456789abcdef00000104")
        self.assertEqual([(r["state"], r["wake"]) for r in state["recipients"]], [("delivered", None)])
        self.assertEqual(sorted(p.name for p in DEMUX.receipts_directory(self.root, self.M1).iterdir()),
                         [f"{both['delivery_id']}.delivered.json", "sent.json"])

    def test_an_unwritable_receipts_directory_is_counted_and_fails_the_run(self):
        self.history()
        receipts = self.root / "runtime" / "receipts"
        receipts.mkdir(parents=True)
        receipts.chmod(0o500)
        self.addCleanup(receipts.chmod, 0o700)
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual(result, 3)
        self.assertEqual(json.loads(output), {**self.EXPECTED, "failed": 7,
                                              "written": dict.fromkeys(self.EXPECTED["written"], 0)})
        self.assertIn("receipt not written", error)
        self.assertNotIn("Iwo", error)
        self.assertEqual(sorted(receipts.iterdir()), [])

    def test_history_gets_its_files_once_and_the_sources_stay(self):
        copies = self.history()
        before = self.sources()
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual((result, error), (0, ""))
        self.assertEqual(json.loads(output), self.EXPECTED)
        self.assertNotIn("Iwo", output)
        self.assertEqual(self.sources(), before)
        directory = DEMUX.receipts_directory(self.root, self.M1)
        delivered = json.loads((directory / f"{copies['read']['delivery_id']}.delivered.json").read_text())
        self.assertEqual((delivered["wake"], delivered["at"], delivered["name"], delivered["channel"]),
                         (None, "2026-10-20T09:00:00Z", "lena", "3"))
        self.assertEqual(delivered["sent_to"], REACH)
        self.assertNotIn("text", delivered)
        read = json.loads((directory / f"{copies['read']['delivery_id']}.read.json").read_text())
        self.assertEqual((read["read_at"], read["name"], read["channel"], read["provider_session_id"]),
                         ("2026-10-20T10:00:00Z", "lena", "3", self.session))
        skipped = json.loads((directory / f"{'4' * 24}.skipped.json").read_text())
        self.assertEqual((skipped["name"], skipped["channel"], skipped["policy"], skipped["at"]),
                         ("igor", "7", "off", "2026-10-20T09:00:05Z"))
        sent = json.loads((directory / "sent.json").read_text())
        self.assertEqual((sent["sent_to"], sent["at"], sent["bus"]), (REACH, "2026-10-20T09:00:00Z", None))
        spoken = json.loads((directory / f"playback.{self.TICKET}.json").read_text())
        self.assertEqual((spoken["state"], spoken["spoken"], spoken["at"]), ("spoken", True, "2026-10-20T10:05:00Z"))
        self.assertEqual(sorted(p.name for p in DEMUX.receipts_directory(self.root, self.M2).iterdir()),
                         [f"{copies['unread']['delivery_id']}.delivered.json"])
        self.assertEqual(sorted(p.name for p in DEMUX.receipts_directory(self.root, self.M3).iterdir()),
                         [f"{copies['gone']['delivery_id']}.read.json"])
        # The second run finds every file and writes nothing.
        receipts = {str(p): p.read_bytes() for p in (self.root / "runtime" / "receipts").rglob("*.json")}
        result, output, _ = self.backfill("--backfill-receipts")
        self.assertEqual(result, 0)
        self.assertEqual(json.loads(output), {**self.EXPECTED, "existing": 7,
                                              "written": dict.fromkeys(self.EXPECTED["written"], 0)})
        self.assertEqual({str(p): p.read_bytes() for p in (self.root / "runtime" / "receipts").rglob("*.json")},
                         receipts)
        self.assertEqual(self.sources(), before)

    def test_dry_run_reports_the_same_counts_and_writes_nothing(self):
        self.history()
        before = self.sources()
        result, output, error = self.backfill("--backfill-receipts", "--dry-run")
        self.assertEqual((result, error), (0, ""))
        self.assertEqual(json.loads(output), {**self.EXPECTED, "dry_run": True})
        self.assertFalse((self.root / "runtime" / "receipts").exists())
        self.assertEqual(self.sources(), before)
        self.backfill("--backfill-receipts")
        result, output, _ = self.backfill("--backfill-receipts", "--dry-run")
        self.assertEqual(json.loads(output), {**self.EXPECTED, "dry_run": True, "existing": 7,
                                              "written": dict.fromkeys(self.EXPECTED["written"], 0)})

    def test_receipt_state_reads_the_backfilled_history(self):
        copies = self.history()
        self.backfill("--backfill-receipts")
        state = DEMUX.receipt_state(self.root, self.M1)
        self.assertEqual(state["sent_to"], REACH)
        self.assertEqual([(r["name"], r["channel"], r["state"], r["wake"], r["delivered_at"], r["at"])
                          for r in state["recipients"]],
                         [("igor", "7", "skipped", None, None, "2026-10-20T09:00:05Z"),
                          ("lena", "3", "read", None, "2026-10-20T09:00:00Z", "2026-10-20T10:00:00Z")])
        self.assertEqual(state["denominator"], 1)
        self.assertEqual((state["spoken"]["state"], state["spoken"]["spoken"]), ("spoken", True))
        unread = DEMUX.receipt_state(self.root, self.M2)
        self.assertEqual([(r["state"], r["wake"], r["delivery_id"]) for r in unread["recipients"]],
                         [("delivered", None, copies["unread"]["delivery_id"])])
        self.assertEqual((unread["sent_to"], unread["denominator"], unread["spoken"]), (None, 1, None))
        gone = DEMUX.receipt_state(self.root, self.M3)
        self.assertEqual([(r["state"], r["wake"], r["delivered_at"]) for r in gone["recipients"]],
                         [("read", None, None)])

    def test_temporary_and_unreadable_sources_count_as_unmapped(self):
        self.history()
        markers = self.root / "acknowledgments" / self.lease_id
        (markers / ".7777777777777777777777.json.tmp").write_text("{")
        (markers / f"{'8' * 24}.json").write_text("not json")
        (self.root / "runtime" / "reply-playback" / f"{self.M3}.{'d' * 24}.json").write_text("{}")
        result, output, _ = self.backfill("--backfill-receipts", "--dry-run")
        self.assertEqual(result, 0)
        self.assertEqual(json.loads(output)["unmapped"], 4)

    def test_the_command_stands_alone(self):
        for arguments in (("--backfill-receipts", "--provider", "codex", "--session", "s"),
                          ("--backfill-receipts", "--receipts", MESSAGE_ID),
                          ("--backfill-receipts", "--mute-all"),
                          ("--dry-run",)):
            with self.subTest(arguments=arguments), self.assertRaises(SystemExit) as refused:
                self.backfill(*arguments)
            self.assertEqual(refused.exception.code, 2)
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual((result, error), (0, ""))
        self.assertEqual(json.loads(output), {**self.EXPECTED, "unmapped": 0,
                                              "written": dict.fromkeys(self.EXPECTED["written"], 0)})


class UnattributedSkipMarkerTests(ReceiptFixture):
    """A skip marker older than `message_id` finds its copy in the reader's own bus."""

    backfill = BackfillTests.backfill
    M1, M2 = "0123456789abcdef00000201", "0123456789abcdef00000202"
    # Delivery ids the helper computed before `delivery_identity` was shared
    # (`SessionLease.enrich` at e5ce9b41, lease "a" * 32): the identity every
    # earlier marker on disk was written under.
    GOLDEN = {"typed": "d24e62a48a03543421986898", "channel_seal": "faa6f01311def62f140c2c00",
              "channel_revision": "67889437bfb98d212530de99", "terminal_seal": "59d96b9762c4185fde51f9b0"}

    def owner(self):
        owner, refusal = DEMUX.binding_identity("3", {
            "provider": self.provider, "provider_session_id": self.session, "audience": "lena",
            "bus": str(self.bus)})
        self.assertIsNone(refusal)
        return owner

    def typed(self, message_id):
        """A typed channel-0 copy as `send_text_command` publishes it into a reader's bus."""
        owner = self.owner()
        return {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message", **owner,
                "message_id": message_id, "source_event_id": message_id, "source": "typed",
                "text": "Iwo Iwo", "emitted_at": "2026-10-20T09:00:00Z",
                "sender": dict(DEMUX.FOUNDER_SENDER), "recipients": [owner], "origin_channel": "0",
                "target": {"mode": "broadcast", "channel": "0",
                           "recipients": [{"name": "lena", "channel": "3"}]}}

    def publish(self, *rows):
        with self.bus.open("a", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row) + "\n")

    def skip_on_runtime(self):
        """The follower's own chain over the bus with channel 0 off: the runtime's markers."""
        lease = self.lease()
        self.bind()
        self.policy("off")
        normalizer = DEMUX.EvidenceNormalizer()
        skipped = []
        with contextlib.redirect_stdout(io.StringIO()):
            for raw in DEMUX.replay(self.bus):
                for event in DEMUX.normalized_revision_events(raw, normalizer, lease):
                    if not lease.admits_channel_event(event):
                        continue
                    payload = DEMUX.consider(event, name="lena", hear_all=False, drafts=False, debug=False)
                    if payload is None:
                        continue
                    lease.enrich(payload)
                    self.assertFalse(lease.queue_delivery(payload))
                    skipped.append(payload["delivery_id"])
        # This generation also wrote the receipt; history from before W2-00 has none.
        receipts = self.root / "runtime" / "receipts"
        receipts.rename(self.home / "runtime-receipts")
        return skipped

    def without_message_id(self, delivery_id):
        """Rewrite one marker the way a helper older than W2-00 left it: no `message_id`."""
        path = DEMUX.broadcast_skip_path(self.root, self.lease_id, delivery_id)
        row = json.loads(path.read_text())
        self.assertEqual(row.pop("message_id"), self.M1)
        DEMUX.atomic_json(path, row)

    def skipped_file(self, delivery_id):
        return DEMUX.receipts_directory(self.root, self.M1) / f"{delivery_id}.skipped.json"

    def test_one_delivery_id_is_computed_by_one_function_with_the_previous_identity(self):
        lease = mock.Mock(lease_id="a" * 32, provider="codex", provider_session_id="receipt-session",
                          bus="/fixture/bus.jsonl")
        take = "agent-channel-0-previous-take"
        cases = {
            "typed": {"kind": "message", "source_event_id": "0" * 23 + "1", "message_id": MESSAGE_ID,
                      "audience": "lena", "producer_schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA},
            "channel_seal": {"kind": "message", "session_id": take,
                             "message_id": DEMUX.channel_message_identity(take), "status": DEMUX.SEALED,
                             "source_event_id": "e" * 24, "audience": "*"},
            "channel_revision": {"kind": "draft", "session_id": take,
                                 "message_id": DEMUX.channel_message_identity(take), "status": "live",
                                 "reducer_revision": 2, "reducer_action": "record_ledger_revision",
                                 "source_event_id": "d" * 24, "audience": "*"},
            "terminal_seal": {"kind": "message", "session_id": "dictation-1",
                              "producer_schema": DEMUX.EVIDENCE_SCHEMA,
                              "reducer_action": DEMUX.DIRECT_TERMINAL, "reducer_revision": 3,
                              "status": DEMUX.SEALED, "source_event_id": "c" * 24, "audience": "lena"},
        }
        for name, payload in cases.items():
            with self.subTest(case=name):
                enriched = dict(payload)
                DEMUX.SessionLease.enrich(lease, enriched)
                self.assertEqual(enriched["delivery_id"], self.GOLDEN[name])
                self.assertEqual(DEMUX.delivery_identity("a" * 32, payload), self.GOLDEN[name])
                # The reducer phase, not the row, keys channel takes and terminal seals.
                self.assertEqual(DEMUX.delivery_phase_identity(payload) == payload["source_event_id"],
                                 name == "typed")

    def test_a_marker_without_message_id_gets_the_same_file_as_one_with_it(self):
        self.publish(self.typed(self.M1))
        [delivery] = self.skip_on_runtime()
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual((result, error), (0, ""))
        current = json.loads(output)
        self.assertEqual((current["written"]["skipped"], current["mapped_by_bus"], current["unmapped"]), (1, 0, 0))
        expected = self.skipped_file(delivery).read_bytes()
        (self.root / "runtime" / "receipts").rename(self.home / "receipts-from-current-marker")

        self.without_message_id(delivery)
        sources = BackfillTests.sources(self)
        result, output, error = self.backfill("--backfill-receipts")
        self.assertEqual((result, error), (0, ""))
        backfilled = json.loads(output)
        self.assertEqual((backfilled["written"]["skipped"], backfilled["mapped_by_bus"], backfilled["unmapped"],
                          backfilled["failed"]), (1, 1, 0, 0))
        self.assertEqual(self.skipped_file(delivery).read_bytes(), expected)
        self.assertEqual(BackfillTests.sources(self), sources, "the marker and the bus are only read")
        state = DEMUX.receipt_state(self.root, self.M1)
        self.assertEqual([(r["name"], r["state"]) for r in state["recipients"]], [("lena", "skipped")])
        self.assertEqual(state["denominator"], 0, "a skipped agent is shown, never counted")

        result, output, _ = self.backfill("--backfill-receipts")
        again = json.loads(output)
        self.assertEqual((result, again["existing"], again["written"]["skipped"], again["failed"]), (0, 1, 0, 0))
        self.assertEqual(self.skipped_file(delivery).read_bytes(), expected)

    def test_dry_run_counts_the_mapping_and_writes_nothing(self):
        self.publish(self.typed(self.M1))
        [delivery] = self.skip_on_runtime()
        self.without_message_id(delivery)
        result, output, _ = self.backfill("--backfill-receipts", "--dry-run")
        planned = json.loads(output)
        self.assertEqual((result, planned["written"]["skipped"], planned["mapped_by_bus"], planned["dry_run"]),
                         (0, 1, 1, True))
        self.assertFalse((self.root / "runtime" / "receipts").exists())

    def test_a_marker_whose_bus_lost_the_copy_stays_unmapped(self):
        self.publish(self.typed(self.M1))
        [delivery] = self.skip_on_runtime()
        self.without_message_id(delivery)
        # The bus rotated: only a later message is left in it.
        self.bus.write_text("")
        self.publish(self.typed(self.M2))
        result, output, _ = self.backfill("--backfill-receipts")
        rotated = json.loads(output)
        self.assertEqual((result, rotated["unmapped"], rotated["mapped_by_bus"], rotated["written"]["skipped"]),
                         (0, 1, 0, 0))
        self.assertFalse(self.skipped_file(delivery).exists())
        # A bus that is gone altogether is the same: nothing to tie the marker to.
        self.bus.rename(self.home / "rotated-away.jsonl")
        result, output, _ = self.backfill("--backfill-receipts")
        self.assertEqual((result, json.loads(output)["unmapped"]), (0, 1))

    def test_a_delivery_id_matching_two_messages_stays_unmapped(self):
        self.publish(self.typed(self.M1))
        [delivery] = self.skip_on_runtime()
        self.without_message_id(delivery)
        # Not reachable from today's publishers (a typed copy's phase is its
        # own message id); the rule is that an ambiguous match is not a match.
        with mock.patch.object(DEMUX, "reader_copies_in_bus", return_value={delivery: {self.M1, self.M2}}):
            result, output, _ = self.backfill("--backfill-receipts")
        ambiguous = json.loads(output)
        self.assertEqual((result, ambiguous["unmapped"], ambiguous["mapped_by_bus"]), (0, 1, 0))
        self.assertFalse(self.skipped_file(delivery).exists())

    def test_a_copy_of_another_reader_never_maps_a_marker(self):
        # Igor's copy in the same bus: admitted to igor's lease only, so lena's
        # marker is not tied to it even if the ids were made to collide.
        igor = dict(self.typed(self.M1))
        stranger = {**self.owner(), "provider_session_id": "igor-session",
                    "lease_id": DEMUX.lease_identifier(self.provider, "igor-session"), "audience": "igor",
                    "name": "igor"}
        igor.update(stranger, recipients=[stranger])
        self.publish(igor)
        copies = DEMUX.reader_copies_in_bus(
            self.bus, provider=self.provider, provider_session_id=self.session, lease_id=self.lease_id,
            name="lena", wanted={DEMUX.delivery_identity(self.lease_id, {**igor, "kind": "message"})})
        self.assertEqual(copies, {})


if __name__ == "__main__":
    unittest.main()
