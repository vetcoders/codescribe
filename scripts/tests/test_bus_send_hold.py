"""Held deliveries never enter an agent's mailbox before their release."""
import argparse
import contextlib
import datetime
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
import unittest.mock as mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_send_hold", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)
APPLIED = DEMUX.held_control_applied

NOW = 1792080000.0
RECEIPT = {"stream_id": "fixture", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}


def stamp(seconds):
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).isoformat(timespec="microseconds")


class SendHoldTests(unittest.TestCase):
    def setUp(self):
        scratch = Path(__file__).resolve().parents[2] / "build" / "test-scratch"
        scratch.mkdir(parents=True, exist_ok=True)
        temp = tempfile.TemporaryDirectory(prefix="send-hold-", dir=scratch)
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()
        self.provider, self.session = "codex", "hold-session"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        metadata = mock.patch.object(DEMUX, "NativeProviderModelReader", return_value=None)
        metadata.start()
        self.addCleanup(metadata.stop)
        # CLI tests stand in for the follower; the confirmation tests below use the real check.
        applied = mock.patch.object(DEMUX, "held_control_applied", return_value=True)
        applied.start()
        self.addCleanup(applied.stop)

    def args(self, **overrides):
        values = dict(bridge_home=self.root, bus=self.bus, bus_overridden=True,
                      provider=self.provider, session=self.session, lease=self.lease_id,
                      channel="2", read_limit=8, read_bytes=65536, lease_ttl=120,
                      hold_seconds=10, attach_file=[])
        values.update(overrides)
        return argparse.Namespace(**values)

    def lease(self):
        lease = DEMUX.SessionLease(root=self.root, provider=self.provider,
            provider_session_id=self.session, name="lena", bus=self.bus, requested_id=None,
            ttl_seconds=120, follow_from_end=False, coalesce=True)
        self.addCleanup(lambda: lease.close() if lease.lock_descriptor is not None else None)
        return lease

    def payload(self, lease, index=1, **overrides):
        row = {"schema": DEMUX.EVENT_SCHEMA, "kind": "message",
               "producer_schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA,
               "source_event_id": f"{index:024x}", "message_id": f"{index:024x}",
               "audience": "lena", "channel": "2", "text": "Iwo Iwo Iwo Iwo Iwo",
               "hold_until": stamp(NOW + 10), "state_change_allowed": True}
        row.update(overrides)
        lease.enrich(row)
        return row

    def control(self, payload, kind, index=1, **overrides):
        row = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": kind,
               "identity": DEMUX.held_identity(payload), "control_id": f"c{index:023x}",
               "provider": self.provider, "provider_session_id": self.session,
               "lease_id": self.lease_id, "bus": str(self.bus), "emitted_at": stamp(NOW + 1)}
        row.update(overrides)
        return row

    def events(self):
        path = DEMUX.follower_paths(self.root, self.lease_id)[1]
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def policy(self, value):
        DEMUX.atomic_json(DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus),
            {"schema": DEMUX.BROADCAST_POLICY_SCHEMA, "provider": self.provider,
             "provider_session_id": self.session, "lease_id": self.lease_id,
             "bus": str(self.bus), "policy": value, "emitted_at": DEMUX.utc_now()})

    def test_hold_zero_is_an_ordinary_delivery(self):
        lease = self.lease()
        row = self.payload(lease)
        row.pop("hold_until")
        self.assertTrue(lease.queue_delivery(row))
        self.assertEqual(lease.held, {})
        self.assertEqual(lease.pending[row["delivery_id"]], row)

    def test_sync_context_survives_held_revision_and_expiry_without_changing_ownership(self):
        lease = self.lease()
        peers = [{"channel": "5", "audience": "tomek", "provider": "codex",
                  "provider_session_id": "peer-session", "lease_id": "b" * 32}]
        row = self.payload(lease, also_sent_to=peers, also_sent_to_basis="sync_selection")
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            self.assertFalse(lease.queue_delivery(row))
            self.assertEqual(lease.pending, {})
            lease.control_delivery(self.control(row, "revise", text="Corrected held text"))
        with mock.patch.object(DEMUX.time, "time", return_value=NOW + 11):
            ready = lease.ready_held()
            self.assertEqual(len(ready), 1)
            self.assertTrue(lease.queue_delivery(ready[0]))
        pending = list(lease.pending.values())
        self.assertEqual(len(pending), 1)
        projection = DEMUX.conversation_envelope(pending[0])
        self.assertEqual(projection["text"], "Corrected held text")
        self.assertEqual(projection["also_sent_to"], peers)
        self.assertEqual(projection["also_sent_to_basis"], "sync_selection")
        self.assertEqual(projection["lease_id"], self.lease_id)
        self.assertEqual(projection["provider_session_id"], self.session)

    def test_future_row_parks_without_publication_or_lifecycle_bell(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW), \
                mock.patch.object(DEMUX, "lifecycle_message_received") as bell, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertFalse(lease.queue_delivery(row))
        self.assertEqual(output.getvalue(), "")
        self.assertEqual(lease.pending, {})
        self.assertEqual(list(lease.held), [row["delivery_id"]])
        bell.assert_not_called()
        self.assertEqual(self.events(), [])

    def test_cross_owner_retirement_cannot_strand_held_delivery(self):
        lease = self.lease()
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(self.payload(lease))
        with self.assertRaisesRegex(OSError, "held deliveries"):
            DEMUX.require_drained_lease(self.root, self.lease_id)
        DEMUX.require_drained_lease(self.root, self.lease_id, lenient_catch_up=True)

    def test_revise_then_expiry_promotes_once_with_latest_text_and_history(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            revise = self.control(row, "revise", text="corrected")
            self.assertEqual(lease.control_delivery(revise), [])
            lease.control_delivery(revise)
            self.assertEqual(lease.held[row["delivery_id"]]["revisions"], [row["text"]])
            self.assertEqual(lease.ready_held(), [])
        with mock.patch.object(DEMUX.time, "time", return_value=NOW + 11):
            ready = lease.ready_held()
            self.assertEqual(len(ready), 1)
            self.assertTrue(lease.queue_delivery(ready[0]))
            self.assertFalse(lease.queue_delivery(ready[0]))
            self.assertEqual(lease.ready_held(), [])
        self.assertEqual(lease.pending[row["delivery_id"]]["text"], "corrected")
        self.assertNotIn("revisions", lease.pending[row["delivery_id"]])
        self.assertEqual(lease.held, {})

    def test_withdraw_is_text_free_one_event_and_never_an_ack(self):
        lease = self.lease()
        row = self.payload(lease)
        withdraw = self.control(row, "withdraw")
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.control_delivery(withdraw)
            with contextlib.redirect_stderr(io.StringIO()):
                lease.control_delivery(withdraw)
            self.assertFalse(lease.queue_delivery(row))
        self.assertEqual((lease.pending, lease.held), ({}, {}))
        events = self.events()
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["kind"], "delivery_withdrawn")
        self.assertNotIn("text", events[0])
        self.assertNotIn("revisions", events[0])
        self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, row["delivery_id"]))

    def test_release_uses_the_ordinary_queue_immediately(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            ready = lease.control_delivery(self.control(row, "release"))
            self.assertEqual(len(ready), 1)
            self.assertTrue(lease.queue_delivery(ready[0]))
            self.assertEqual(lease.held, {})
            self.assertEqual(list(lease.pending), [row["delivery_id"]])

    def test_restart_preserves_hold_and_revision_then_expires_once(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.control_delivery(self.control(row, "revise", text="after restart"))
            lease.close()
            resumed = self.lease()
            self.assertEqual(resumed.ready_held(), [])
        with mock.patch.object(DEMUX.time, "time", return_value=NOW + 11):
            self.assertTrue(resumed.queue_delivery(resumed.ready_held()[0]))
            resumed.close()
            resumed = self.lease()
            self.assertEqual(resumed.ready_held(), [])
            self.assertEqual(len(resumed.pending), 1)
            self.assertEqual(resumed.pending[row["delivery_id"]]["text"], "after restart")

    def test_restart_after_withdraw_does_not_resurrect_or_duplicate_receipt(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.control_delivery(self.control(row, "withdraw"))
            lease.close()
            resumed = self.lease()
            self.assertFalse(resumed.queue_delivery(row))
        self.assertEqual((resumed.held, resumed.pending), ({}, {}))
        self.assertEqual(len(self.events()), 1)

    def test_restart_recovers_a_release_before_pending_was_committed(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.control_delivery(self.control(row, "release"))
            lease.close()
            resumed = self.lease()
            self.assertTrue(resumed.queue_delivery(resumed.ready_held()[0]))
        self.assertEqual(len(resumed.pending), 1)
        self.assertEqual(resumed.held, {})

    def test_recovery_rejects_foreign_held_owner_without_overwriting_state(self):
        lease = self.lease()
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(self.payload(lease))
            lease.close()
        path = self.root / "leases" / f"{self.lease_id}.json"
        state = DEMUX.read_json(path)
        next(iter(state["held"].values()))["provider_session_id"] = "foreign"
        DEMUX.atomic_json(path, state)
        original = path.read_bytes()
        with self.assertRaisesRegex(ValueError, "invalid held deliveries"):
            self.lease()
        self.assertEqual(path.read_bytes(), original)

    def test_unknown_or_delivered_control_is_ignored_with_one_stderr_line(self):
        lease = self.lease()
        row = self.payload(lease)
        for kind in ("revise", "withdraw", "release"):
            with contextlib.redirect_stderr(io.StringIO()) as error:
                self.assertEqual(lease.control_delivery(self.control(row, kind, text="no")), [])
            self.assertEqual(len(error.getvalue().splitlines()), 1)
        row.pop("hold_until")
        lease.queue_delivery(row)
        with contextlib.redirect_stderr(io.StringIO()) as error:
            lease.control_delivery(self.control(row, "withdraw"))
        self.assertEqual(len(error.getvalue().splitlines()), 1)
        self.assertEqual(len(lease.pending), 1)

    def test_broadcast_off_precedes_hold_and_quiet_stays_held(self):
        lease = self.lease()
        self.policy("off")
        row = self.payload(lease, channel="0")
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            self.assertFalse(lease.queue_delivery(row))
            self.assertEqual(lease.held, {})
            self.assertEqual(self.events()[0]["kind"], "broadcast_skipped")
            self.policy("quiet")
            quiet = self.payload(lease, 2, channel="0")
            self.assertFalse(lease.queue_delivery(quiet))
            self.assertEqual(len(lease.held), 1)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW + 11):
            self.assertTrue(lease.queue_delivery(lease.ready_held()[0]))
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [])
        self.assertEqual(len(DEMUX.unread_pending(self.args(), include_quiet=True)[1]), 1)

    def test_switching_broadcast_off_while_held_skips_instead_of_promoting(self):
        lease = self.lease()
        row = self.payload(lease, channel="0")
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            self.policy("off")
            self.assertFalse(lease.queue_delivery(lease.ready_held()[0]))
        self.assertEqual((lease.held, lease.pending), ({}, {}))
        self.assertEqual([r["kind"] for r in self.events()], ["broadcast_skipped"])

    def test_receipts_expose_only_held_identity_and_deadline(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        expected = [{"identity": row["message_id"], "hold_until": row["hold_until"]}]
        self.assertEqual(lease.attach_receipt()["held"], expected)
        for command in (DEMUX.read_pending_command, DEMUX.status_command):
            with mock.patch.object(DEMUX, "emit") as emitted:
                command(self.args())
            self.assertEqual(emitted.call_args.args[0]["held"], expected)
            self.assertIn("broadcast", emitted.call_args.args[0])

    def test_send_text_hold_receipts_and_zero_omission(self):
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        for seconds in (0, 10):
            with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX.time, "time", return_value=NOW), \
                    mock.patch.object(DEMUX, "emit") as emitted, \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"typed"))):
                self.assertEqual(DEMUX.send_text_command(self.args(hold_seconds=seconds)), 0)
            row, receipt = publish.call_args.args[1], emitted.call_args.args[0]
            for item in (row, receipt, receipt["recipients"][0]):
                if seconds:
                    self.assertEqual(item["hold_until"], stamp(NOW + 10))
                else:
                    self.assertNotIn("hold_until", item)

    def raw_message(self, **overrides):
        row = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
               "source": "typed", "message_id": f"{1:024x}", "source_event_id": f"{1:024x}",
               "audience": "lena", "channel": "2", "text": "original held text",
               "hold_until": stamp(NOW + 10), "emitted_at": stamp(NOW)}
        row.update(overrides)
        return row

    def run_follower(self, batches):
        args = self.args(name="lena", all=False, become=False, follow=True, from_start=True,
            coalesce=True, on_seal="fixture-hook", wakeup="codex-queue", drafts=True,
            debug=False, once=False, interval=0.01,
            follower_events=DEMUX.follower_paths(self.root, self.lease_id)[1], follower_channel="2")
        args.follower_events.parent.mkdir(parents=True, exist_ok=True)
        index = 0
        clock = [NOW]
        def read(_path, offset):
            nonlocal index
            if index == len(batches):
                raise KeyboardInterrupt
            clock[0], rows = batches[index]
            index += 1
            return [(json.dumps(row), offset + n + 1) for n, row in enumerate(rows)], offset + len(rows)
        with mock.patch.object(DEMUX, "iter_new_lines", side_effect=read), \
                mock.patch.object(DEMUX.time, "time", side_effect=lambda: clock[0]), \
                mock.patch.object(DEMUX, "BusEventTrigger"), \
                mock.patch.object(DEMUX, "NativeQueueWakeup") as native, \
                mock.patch.object(DEMUX, "fire_seal_hook") as hook, \
                contextlib.redirect_stdout(io.StringIO()) as output, \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(DEMUX.run(args), 130)
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        return state, output.getvalue(), native.return_value.enqueue.call_args_list, hook.call_args_list

    def test_follower_has_no_event_stdout_native_or_hook_for_held_row(self):
        state, output, native, hooks = self.run_follower([(NOW, [self.raw_message()])])
        self.assertEqual(len(state["held"]), 1)
        self.assertEqual(state["pending"], [])
        self.assertNotIn("original held text", output)
        self.assertEqual((native, hooks), ([], []))
        self.assertFalse(any(row.get("kind") == "message" for row in self.events()))

    def test_ten_second_hold_never_starts_agent_delivery_during_countdown(self):
        self.assert_countdown_is_private(10)

    def test_fifteen_second_hold_never_starts_agent_delivery_during_countdown(self):
        self.assert_countdown_is_private(15)

    def assert_countdown_is_private(self, seconds):
        row = self.raw_message(hold_until=stamp(NOW + seconds))
        state, output, native, hooks = self.run_follower([
            (NOW, [row]), (NOW + seconds - 5, []), (NOW + seconds - 0.001, [])])
        self.assertEqual(state["pending"], [])
        self.assertEqual(len(state["held"]), 1)
        self.assertNotIn(row["text"], output)
        self.assertEqual((native, hooks), ([], []))
        self.assertFalse(any(event.get("kind") == "message" for event in self.events()))

    def test_withdraw_written_five_seconds_before_deadline_wins_even_if_read_after_it(self):
        row = self.raw_message(hold_until=stamp(NOW + 15))
        control = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "withdraw",
            "identity": row["message_id"], "control_id": "a" * 24,
            "provider": self.provider, "provider_session_id": self.session,
            "lease_id": self.lease_id, "bus": str(self.bus), "emitted_at": stamp(NOW + 10)}
        # The real follower loop must drain bus controls before promoting due rows.
        state, output, native, hooks = self.run_follower([
            (NOW, [row]), (NOW + 16, [control]), (NOW + 17, [])])
        self.assertEqual((state["pending"], state["held"]), ([], {}))
        self.assertNotIn(row["text"], output)
        self.assertEqual((native, hooks), ([], []))
        self.assertEqual([event["kind"] for event in self.events()
                          if event["kind"] != "attach"], ["delivery_withdrawn"])

    def test_fifteen_second_deadline_delivers_once_without_preparing_an_agent_early(self):
        row = self.raw_message(hold_until=stamp(NOW + 15))
        state, _, native, hooks = self.run_follower([
            (NOW, [row]), (NOW + 14.999, []), (NOW + 15, []), (NOW + 16, [])])
        self.assertEqual(state["held"], {})
        self.assertEqual([item["text"] for item in state["pending"]], [row["text"]])
        self.assertEqual((len(native), len(hooks)), (1, 1))

    def test_resumed_follower_drains_withdraw_before_expired_unread_message(self):
        row = self.raw_message(hold_until=stamp(NOW + 15))
        control = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "withdraw",
            "identity": row["message_id"], "control_id": "a" * 24,
            "provider": self.provider, "provider_session_id": self.session,
            "lease_id": self.lease_id, "bus": str(self.bus), "emitted_at": stamp(NOW + 10)}
        state, output, native, hooks = self.run_follower([
            (NOW + 16, [row, control]), (NOW + 17, [])])
        self.assertEqual((state["pending"], state["held"]), ([], {}))
        self.assertNotIn(row["text"], output)
        self.assertEqual((native, hooks), ([], []))
        self.assertEqual(len(state["withdrawn_held"]), 1)

    def test_resumed_follower_publishes_only_revised_text_after_expired_unread_message(self):
        row = self.raw_message(hold_until=stamp(NOW + 15))
        control = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "revise",
            "identity": row["message_id"], "control_id": "a" * 24,
            "provider": self.provider, "provider_session_id": self.session,
            "lease_id": self.lease_id, "bus": str(self.bus), "emitted_at": stamp(NOW + 10),
            "text": "corrected before the deadline"}
        state, output, native, hooks = self.run_follower([
            (NOW + 16, [row, control]), (NOW + 17, [])])
        self.assertEqual(state["held"], {})
        self.assertEqual([item["text"] for item in state["pending"]], [control["text"]])
        self.assertNotIn(row["text"], output)
        self.assertEqual((len(native), len(hooks)), (1, 1))

    def test_resumed_follower_delivers_expired_unread_message_once_without_control(self):
        row = self.raw_message(hold_until=stamp(NOW + 15))
        state, _, native, hooks = self.run_follower([
            (NOW + 16, [row]), (NOW + 17, [])])
        self.assertEqual(state["held"], {})
        self.assertEqual([item["text"] for item in state["pending"]], [row["text"]])
        self.assertEqual((len(native), len(hooks)), (1, 1))

    def test_follower_expiry_and_release_share_one_publication_path(self):
        row = self.raw_message()
        control = {"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "revise",
            "identity": row["message_id"], "control_id": "a" * 24, "text": "latest held text",
            "provider": self.provider, "provider_session_id": self.session,
            "lease_id": self.lease_id, "bus": str(self.bus)}
        state, output, native, hooks = self.run_follower([
            (NOW, [row]), (NOW + 1, [control]), (NOW + 11, []), (NOW + 12, [])])
        self.assertEqual(state["held"], {})
        self.assertEqual([p["text"] for p in state["pending"]], ["latest held text"])
        self.assertNotIn("original held text", output)
        self.assertEqual(len(native), 1)
        self.assertEqual(len(hooks), 1)
        self.assertEqual([r["text"] for r in self.events() if r.get("kind") == "message"], ["latest held text"])

    def test_voice_manual_revision_changes_held_text_not_a_new_delivery(self):
        voice = {"schema": DEMUX.CLEAN_SCHEMA, "status": DEMUX.SEALED,
            "session_id": "voice-take", "utterance_id": "voice-id", "audience": "lena",
            "sequence": 1, "text": "voice original", "hold_until": stamp(NOW + 10)}
        edit = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": "voice-take",
            "utterance_id": "voice-id", "reducer_action": "apply_manual_edit",
            "reducer_revision": 2, "sequence": 2, "rendered_text": "voice correction"}
        state, output, native, hooks = self.run_follower([
            (NOW, [voice]), (NOW + 1, [edit]), (NOW + 11, [])])
        self.assertEqual([p["text"] for p in state["pending"]], ["voice correction"])
        self.assertNotIn("voice original", output)
        self.assertEqual((len(native), len(hooks)), (1, 1))

    def test_joined_voice_twins_preserve_hold_and_numeric_identity_without_double_delivery(self):
        session = "agent-channel-2-hold-fixture"
        # Channel evidence is admitted only through its frozen owner list,
        # the way the Rust publisher stamps every channel row.
        owner = {"provider": self.provider, "provider_session_id": self.session,
                 "lease_id": self.lease_id, "bus": str(self.bus)}
        preview = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session,
            "recipients": [owner],
            "audience": "lena", "reducer_revision": 3, "sequence": 4,
            "reducer_action": DEMUX.TERMINAL_SEAL, "rendered_text": "joined voice",
            "occurrence_session_id": "capture", "capture_epoch": 1,
            "sample_start": 0, "sample_end": 16000, "document_index": 0}
        closing = {**preview, "status": DEMUX.SEALED, "terminal": True,
                   "sequence": 5, "utterance_id": 42, "hold_until": stamp(NOW + 10)}
        clean = {**closing, "schema": DEMUX.CLEAN_SCHEMA, "sequence": 6, "text": "joined voice"}
        boundary = {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "channel": "2",
                    "session_id": session, "state": "sealed", "emitted_at": stamp(NOW)}
        state, _output, native, hooks = self.run_follower([
            (NOW, [preview, closing, clean, boundary])])
        self.assertEqual(len(state["held"]), 1)
        self.assertEqual(state["pending"], [])
        held = next(iter(state["held"].values()))
        self.assertEqual((held["utterance_id"], held["hold_until"]), (42, closing["hold_until"]))
        # The preview reaches the native queue boundary as every draft does;
        # the real queue drops non-terminal kinds. No seal may cross it.
        self.assertFalse(any(call.args[0].get("kind") in DEMUX.TERMINAL_KINDS for call in native))
        self.assertEqual(hooks, [])
        self.assertFalse(any(row.get("kind") == "seal" for row in self.events()))

    def test_closing_metadata_survives_a_restart_before_channel_boundary(self):
        session = "agent-channel-2-restart-fixture"
        normalizer = DEMUX.EvidenceNormalizer()
        row = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session,
            "reducer_revision": 3, "reducer_action": DEMUX.TERMINAL_SEAL,
            "rendered_text": "voice", "audience": "lena", "sequence": 1}
        normalizer.normalize(row)
        normalizer.normalize({**row, "status": DEMUX.SEALED, "terminal": True,
            "utterance_id": 0, "hold_until": stamp(NOW + 10), "sequence": 2})
        retained = normalizer.channel_documents()
        self.assertIn(retained[session]["status"], DEMUX.LIVE_STATUSES)
        restored = DEMUX.EvidenceNormalizer()
        restored.restore_channel_documents(list(retained.values()))
        restored.normalize({"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "channel": "2",
                            "session_id": session, "state": "sealed"})
        sealed = restored.pop_flushes()[0]
        self.assertEqual(sealed["utterance_id"], 0)
        self.assertEqual(sealed["hold_until"], stamp(NOW + 10))

    def closing_voice_checkpoint(self):
        lease = self.lease()
        owner = {key: getattr(lease, key) for key in ("provider", "provider_session_id", "lease_id", "bus")}
        session = "agent-channel-2-checkpoint-fixture"
        closing = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session,
            "recipients": [owner], "audience": "lena", "capture_epoch": 1,
            "reducer_revision": 3, "reducer_action": DEMUX.TERMINAL_SEAL,
            "rendered_text": "retained closing voice", "sequence": 1,
            "status": DEMUX.SEALED, "terminal": True, "closing_decision": DEMUX.CLOSING_PUBLISH,
            "utterance_id": 42, "hold_until": stamp(NOW + 10)}
        normalizer = DEMUX.EvidenceNormalizer()
        DEMUX.normalized_revision_events(json.dumps(closing), normalizer)
        self.bus.write_text(json.dumps(closing) + "\n")
        lease.unclosed_channel_messages = normalizer.channel_documents()
        lease.persist(active=True, cursor=self.bus.stat().st_size)
        boundary = {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, **owner, "channel": "2",
            "session_id": session, "state": "sealed", "publication": DEMUX.CONTROLLER_PUBLICATION,
            "emitted_at": stamp(NOW)}
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        return lease, boundary

    def assert_control_after_closing_checkpoint(self, kind):
        lease, boundary = self.closing_voice_checkpoint()
        with self.bus.open("a") as bus:
            bus.write(json.dumps(boundary) + "\n")
        lease.close()
        published = []

        def publish_then_follow(path, event, **kwargs):
            self.assertEqual(path, self.bus)
            published.append(event)
            # Resume the actual follower at the checkpoint. It has not yet read
            # the boundary when preflight validates the requested control.
            self.run_follower([(NOW + 2, [boundary, event])])
            return RECEIPT

        args = self.args(revise=None, withdraw=None, release=None)
        setattr(args, kind, boundary["session_id"])
        with mock.patch.object(DEMUX, "publish_reply_event", side_effect=publish_then_follow), \
                mock.patch.object(DEMUX, "held_control_applied", side_effect=APPLIED), \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 2), \
                mock.patch.object(DEMUX, "emit"), \
                mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"corrected closing voice"))):
            self.assertEqual(DEMUX.delivery_control_command(args), 0)
        self.assertEqual(len(published), 1)
        self.assertEqual((published[0]["session_id"], published[0]["utterance_id"]),
                         (boundary["session_id"], 42))
        state = DEMUX.read_json(lease.path)
        if kind == "release":
            self.assertEqual(state["held"], {})
            self.assertEqual([row["text"] for row in state["pending"]], ["retained closing voice"])
        elif kind == "withdraw":
            self.assertEqual((state["held"], state["pending"]), ({}, []))
            self.assertEqual(len(state["withdrawn_held"]), 1)
        else:
            self.assertEqual(state["pending"], [])
            self.assertEqual([row["text"] for row in state["held"].values()], ["corrected closing voice"])

    def test_send_now_after_checkpoint_between_closing_snapshot_and_boundary(self):
        self.assert_control_after_closing_checkpoint("release")

    def test_cancel_after_checkpoint_between_closing_snapshot_and_boundary(self):
        self.assert_control_after_closing_checkpoint("withdraw")

    def test_edit_after_checkpoint_between_closing_snapshot_and_boundary(self):
        self.assert_control_after_closing_checkpoint("revise")

    def test_closing_checkpoint_without_an_owned_boundary_is_not_deliverable(self):
        lease, boundary = self.closing_voice_checkpoint()
        for tail in ([], [{**boundary, "provider_session_id": "foreign"}]):
            with self.bus.open("a") as bus:
                for row in tail:
                    bus.write(json.dumps(row) + "\n")
            before = lease.path.read_bytes()
            with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                    mock.patch.object(DEMUX, "held_control_applied") as applied, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX.time, "time", return_value=NOW + 2):
                with self.assertRaisesRegex(ValueError, "no longer held"):
                    DEMUX.delivery_control_command(self.args(
                        revise=None, withdraw=boundary["session_id"], release=None))
            publish.assert_not_called()
            applied.assert_not_called()
            self.assertEqual(lease.path.read_bytes(), before)

    def test_foreign_closing_checkpoint_cannot_be_restored_by_an_owned_boundary(self):
        lease, boundary = self.closing_voice_checkpoint()
        document = lease.unclosed_channel_messages[boundary["session_id"]]
        document["recipients"][0]["provider_session_id"] = "foreign"
        lease.persist(active=True)
        with self.bus.open("a") as bus:
            bus.write(json.dumps(boundary) + "\n")
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321):
            with self.assertRaisesRegex(ValueError, "invalid unclosed"):
                DEMUX.delivery_control_command(self.args(
                    revise=None, withdraw=None, release=boundary["session_id"]))
        publish.assert_not_called()

    def test_owned_refusal_suppresses_a_retained_closing_checkpoint(self):
        lease, boundary = self.closing_voice_checkpoint()
        lease.channel_publication_refusals[boundary["session_id"]] = {
            "schema": DEMUX.PUBLICATION_SCHEMA, "status": "channel_publication_refused",
            "reason": "sound_without_speech", "entirely_unwitnessed": True,
            "session_id": boundary["session_id"], "capture_epoch": 1,
            "recipients": [lease.unclosed_channel_messages[boundary["session_id"]]["recipients"][0]],
        }
        lease.persist(active=True)
        with self.bus.open("a") as bus:
            bus.write(json.dumps(boundary) + "\n")
        before = lease.path.read_bytes()
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321):
            with self.assertRaisesRegex(ValueError, "no longer held"):
                DEMUX.delivery_control_command(self.args(
                    revise=None, withdraw=None, release=boundary["session_id"]))
        publish.assert_not_called()
        self.assertEqual(lease.path.read_bytes(), before)

    def test_partial_boundary_does_not_seal_a_retained_closing_checkpoint(self):
        lease, boundary = self.closing_voice_checkpoint()
        with self.bus.open("a") as bus:
            bus.write(json.dumps(boundary))
        before = lease.path.read_bytes()
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321):
            with self.assertRaisesRegex(ValueError, "no longer held"):
                DEMUX.delivery_control_command(self.args(
                    revise=None, withdraw=None, release=boundary["session_id"]))
        publish.assert_not_called()
        self.assertEqual(lease.path.read_bytes(), before)

    def test_expired_closing_checkpoint_cannot_be_cancelled_or_edited(self):
        lease, boundary = self.closing_voice_checkpoint()
        with self.bus.open("a") as bus:
            bus.write(json.dumps(boundary) + "\n")
        before = lease.path.read_bytes()
        for kind in ("withdraw", "revise"):
            args = self.args(revise=None, withdraw=None, release=None)
            setattr(args, kind, boundary["session_id"])
            with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX.time, "time", return_value=NOW + 11), \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"too late"))):
                with self.assertRaisesRegex(ValueError, "message already delivered"):
                    DEMUX.delivery_control_command(args)
            publish.assert_not_called()
        self.assertEqual(lease.path.read_bytes(), before)

    def voice_journal_rows(self, *sessions):
        owner = {"provider": self.provider, "provider_session_id": self.session,
                 "lease_id": self.lease_id, "bus": str(self.bus)}
        rows = []
        for session in sessions:
            rows.extend([
                {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session,
                 "recipients": [owner], "audience": "lena", "reducer_revision": 1,
                 "reducer_action": DEMUX.TERMINAL_SEAL, "rendered_text": "held voice",
                 "sequence": 1, "utterance_id": 42, "status": DEMUX.SEALED,
                 "hold_until": stamp(NOW + 10)},
                {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, **owner, "session_id": session,
                 "channel": "2", "state": "sealed"},
            ])
        return rows

    def test_quiet_broadcast_promotes_without_any_wake(self):
        self.policy("quiet")
        state, output, native, hooks = self.run_follower([
            (NOW, [self.raw_message(origin_channel="0")]), (NOW + 11, [])])
        self.assertEqual(state["held"], {})
        self.assertEqual(len(state["pending"]), 1)
        self.assertNotIn("original held text", output)
        self.assertEqual((native, hooks), ([], []))

    def test_control_cli_validates_owner_and_publishes_same_bus(self):
        self.lease()
        self.bus.write_text(json.dumps(self.raw_message(
            message_id="a" * 24, hold_until=stamp(DEMUX.time.time() + 30))) + "\n")
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        for kind in ("revise", "withdraw", "release"):
            args = self.args(revise=None, withdraw=None, release=None)
            setattr(args, kind, "a" * 24)
            with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX, "emit"), \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"revision"))):
                self.assertEqual(DEMUX.delivery_control_command(args), 0)
                self.assertEqual(publish.call_args.args[0], self.bus)
                event = publish.call_args.args[1]
                self.assertEqual((event["schema"], event["kind"]), (DEMUX.DELIVERY_CONTROL_SCHEMA, kind))
                self.assertEqual((event["provider"], event["provider_session_id"], event["lease_id"]),
                                 (self.provider, self.session, self.lease_id))
                self.assertEqual(event.get("text"), "revision" if kind == "revise" else None)
                args.session = "foreign"
                with self.assertRaises(ValueError):
                    DEMUX.delivery_control_command(args)
                self.assertEqual(publish.call_count, 1)

    def test_held_controls_use_the_lease_checkpoint_without_replaying_old_generations(self):
        lease = self.lease()
        # The checkpoint already consumed this history; controls have no reason
        # to parse it again. A real held payload remains owned by the follower.
        self.bus.write_text("historical bytes\n" * 10000)
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.persist(active=True, cursor=self.bus.stat().st_size)
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        before = lease.path.read_bytes()
        for kind in ("withdraw", "release", "revise"):
            args = self.args(revise=None, withdraw=None, release=None)
            setattr(args, kind, row["message_id"])
            with mock.patch.object(DEMUX, "replay", side_effect=AssertionError("historical replay")), \
                    mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX, "emit"), \
                    mock.patch.object(DEMUX.time, "time", return_value=NOW + 5), \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"corrected"))):
                self.assertEqual(DEMUX.delivery_control_command(args), 0)
                self.assertEqual(publish.call_args.args[1]["message_id"], row["message_id"])
                self.assertEqual(publish.call_args.args[1]["kind"], kind)
        self.assertEqual(lease.path.read_bytes(), before, "Only the follower may advance its checkpoint")

    def test_unread_release_after_checkpoint_prevents_a_false_withdraw_success(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        self.bus.write_text(json.dumps(self.control(row, "release")) + "\n")
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 2):
            with self.assertRaisesRegex(ValueError, "message already delivered"):
                DEMUX.delivery_control_command(self.args(revise=None, withdraw=row["message_id"], release=None))
        publish.assert_not_called()

    def test_five_seconds_remaining_withdraw_requires_the_real_follower_receipt(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        def publish_then_follow(path, event, **kwargs):
            # The fixture owns its disposable journal; this receipt test must
            # not require a locally built external publisher on a clean runner.
            with path.open("a") as bus:
                bus.write(json.dumps(event) + "\n")
            self.assertFalse(APPLIED(self.root, self.lease_id, "withdraw", event["control_id"]))
            lease.control_delivery(event)
            return RECEIPT

        with mock.patch.object(DEMUX, "publish_reply_event", side_effect=publish_then_follow), \
                mock.patch.object(DEMUX, "held_control_applied", side_effect=APPLIED), \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 5), \
                mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.delivery_control_command(
                self.args(revise=None, withdraw=row["message_id"], release=None)), 0)
        self.assertEqual((lease.pending, lease.held), ({}, {}))
        self.assertEqual(emitted.call_args.args[0]["control_kind"], "withdraw")
        self.assertEqual(len(lease.withdrawn_held), 1)

    def test_already_pending_message_cannot_be_withdrawn_before_its_old_hold_deadline(self):
        lease = self.lease()
        row = self.payload(lease, hold_released=True)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            self.assertTrue(lease.queue_delivery(row))
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 2):
            with self.assertRaisesRegex(ValueError, "message already delivered"):
                DEMUX.delivery_control_command(self.args(revise=None, withdraw=row["message_id"], release=None))
        publish.assert_not_called()

    def test_foreign_retained_payload_refuses_a_control_without_effects(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        lease.held[row["delivery_id"]]["provider_session_id"] = "foreign"
        lease.persist(active=True)
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321):
            with self.assertRaisesRegex(ValueError, "invalid held delivery owner"):
                DEMUX.delivery_control_command(self.args(revise=None, withdraw=row["message_id"], release=None))
        publish.assert_not_called()

    def test_acked_or_withdrawn_identity_refuses_without_publish_or_confirmation_wait(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
            lease.control_delivery(self.control(row, "withdraw"))
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        for withdrawn in (True, False):
            if not withdrawn:
                lease.withdrawn_held.clear()  # the equally unknown, already ACKed case
                lease.persist(active=True)
            for kind in ("withdraw", "release"):
                args = self.args(revise=None, withdraw=None, release=None)
                setattr(args, kind, row["message_id"])
                with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                        mock.patch.object(DEMUX, "held_control_applied") as applied, \
                        mock.patch.object(DEMUX, "live_follower_pid", return_value=321):
                    with self.assertRaisesRegex(ValueError, "no longer held"):
                        DEMUX.delivery_control_command(args)
                publish.assert_not_called()
                applied.assert_not_called()

    def test_partial_unread_append_does_not_prevent_control_of_an_existing_held_row(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        self.bus.write_text('{"schema":"codescribe.transcript.v1"')
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 5), \
                mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.delivery_control_command(
                self.args(revise=None, withdraw=row["message_id"], release=None)), 0)
        self.assertEqual(publish.call_args.args[1]["message_id"], row["message_id"])

    def test_control_cli_refuses_withdraw_or_revise_once_the_deadline_delivered(self):
        # 23 X 10:15: a withdraw written 1.2 s after hold_until exited 0, the
        # composer took the text back, and the agent had already received it.
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        message = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "message_id": "b" * 24,
                   "text": "Iwo", "hold_until": stamp(NOW + 10)}
        self.bus.write_text(json.dumps(message) + "\n")
        def control(kind, at):
            args = self.args(revise=None, withdraw=None, release=None)
            setattr(args, kind, "b" * 24)
            with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX, "emit"), \
                    mock.patch.object(DEMUX.time, "time", return_value=at), \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"revision"))):
                try:
                    return DEMUX.delivery_control_command(args), publish.call_count
                except ValueError as refused:
                    return str(refused), publish.call_count
        self.assertEqual(control("withdraw", NOW + 5), (0, 1))
        for kind in ("withdraw", "revise"):
            for at in (NOW + 9.6, NOW + 11.2):
                self.assertEqual(control(kind, at),
                                 ("message already delivered; it can no longer be changed", 0))
        # Releasing an already delivered row stays a harmless no-op.
        self.assertEqual(control("release", NOW + 11.2), (0, 1))
        # An explicit release hands the row over before its deadline, too.
        with self.bus.open("a") as bus:
            bus.write(json.dumps({"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "release",
                                  "identity": "b" * 24, "lease_id": self.lease_id}) + "\n")
        self.assertEqual(control("withdraw", NOW + 2),
                         ("message already delivered; it can no longer be changed", 0))

    def test_a_release_of_one_capture_leaves_its_label_twin_held(self):
        # Copilot on #182: two held takes share utterance label 42; releasing the
        # first must not make a withdraw of the second report "already delivered".
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        first, second = "agent-channel-2-first", "agent-channel-2-second"
        rows = self.voice_journal_rows(first, second)
        rows.append({"schema": DEMUX.DELIVERY_CONTROL_SCHEMA, "kind": "release", "identity": "42",
                     "session_id": first, "lease_id": self.lease_id, "provider": self.provider,
                     "provider_session_id": self.session, "bus": str(self.bus)})
        self.bus.write_text("".join(json.dumps(row) + "\n" for row in rows))
        args = self.args(revise=None, withdraw=second, release=None)
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX, "emit"), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 2):
            self.assertEqual(DEMUX.delivery_control_command(args), 0)
            self.assertEqual(publish.call_args.args[1]["session_id"], second)
            args.withdraw = first
            with self.assertRaisesRegex(ValueError, "message already delivered"):
                DEMUX.delivery_control_command(args)
            self.assertEqual(publish.call_count, 1)

    def test_control_cli_reports_success_only_after_the_follower_applies_it(self):
        # Copilot on #182: the append receipt is not the agent's view. A withdraw
        # that lands after the follower handed the row over is ignored there.
        self.lease()
        self.bus.write_text(json.dumps(self.raw_message(
            message_id="a" * 24, hold_until=stamp(DEMUX.time.time() + 30))) + "\n")
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        for kind in ("withdraw", "revise"):
            args = self.args(revise=None, withdraw=None, release=None)
            setattr(args, kind, "a" * 24)
            with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX, "emit") as emitted, \
                    mock.patch.object(DEMUX, "HOLD_CONTROL_CONFIRM_SECONDS", 0.2), \
                    mock.patch.object(DEMUX, "held_control_applied", return_value=False), \
                    mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"revision"))):
                with self.assertRaisesRegex(ValueError, "recorded but not confirmed by 1 of 1"):
                    DEMUX.delivery_control_command(args)
                self.assertEqual(publish.call_count, 1)
                emitted.assert_not_called()

    def test_follower_state_confirms_only_the_control_it_applied(self):
        lease = self.lease()
        kept, gone = self.payload(lease, 1), self.payload(lease, 2)
        revise = self.control(kept, "revise", 1, text="corrected")
        withdraw = self.control(gone, "withdraw", 2)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(kept)
            lease.queue_delivery(gone)
            self.assertFalse(APPLIED(self.root, self.lease_id, "revise", revise["control_id"]))
            lease.control_delivery(revise)
            lease.control_delivery(withdraw)
        self.assertTrue(APPLIED(self.root, self.lease_id, "revise", revise["control_id"]))
        self.assertTrue(APPLIED(self.root, self.lease_id, "withdraw", withdraw["control_id"]))
        self.assertFalse(APPLIED(self.root, self.lease_id, "withdraw", revise["control_id"]))
        # The revision stays confirmed after its deadline hands the row over.
        with mock.patch.object(DEMUX.time, "time", return_value=NOW + 11):
            for ready in lease.ready_held():
                lease.queue_delivery(ready)
        self.assertEqual(lease.held, {})
        self.assertTrue(APPLIED(self.root, self.lease_id, "revise", revise["control_id"], "corrected"))
        self.assertFalse(APPLIED(self.root, self.lease_id, "revise", "c" * 24, "another revision"))

    def test_broadcast_controls_fan_out_and_send_text_carries_one_deadline(self):
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        other = self.home / "other.jsonl"
        other.touch()
        peer_id = DEMUX.lease_identifier("junie", "peer")
        DEMUX.atomic_json(self.root / "leases" / f"{peer_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": peer_id, "provider": "junie",
            "provider_session_id": "peer", "bus": str(other), "cursor": 0,
            "held": {}, "pending": []})
        DEMUX.write_channel_binding(self.root, "3", "peer", "junie", "peer", str(other))
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX, "emit") as emitted, \
                mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"broadcast"))):
            DEMUX.send_text_command(self.args(channel="0"))
            deadline = emitted.call_args.args[0]["hold_until"]
            self.assertEqual([call.args[1]["hold_until"] for call in publish.call_args_list], [deadline, deadline])
            for bus in (self.bus, other):
                bus.write_text(json.dumps(self.raw_message(message_id="a" * 24, hold_until=deadline)) + "\n")
            publish.reset_mock()
            DEMUX.delivery_control_command(self.args(channel="0", provider=None, session=None,
                lease=None, revise=None, withdraw="a" * 24, release=None))
            self.assertEqual({call.args[0] for call in publish.call_args_list}, {self.bus, other})

    def test_broadcast_withdraw_skips_a_new_or_opted_out_owner_without_blocking_held_owners(self):
        lease = self.lease()
        row = self.payload(lease)
        with mock.patch.object(DEMUX.time, "time", return_value=NOW):
            lease.queue_delivery(row)
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        other = self.home / "other.jsonl"
        other.touch()
        peer_id = DEMUX.lease_identifier("junie", "peer")
        DEMUX.write_channel_binding(self.root, "3", "peer", "junie", "peer", str(other))
        for opted_out in (False, True):
            # A later join has never seen the row; an opted-out follower has
            # consumed and skipped its copy. Neither owns a held delivery.
            other.write_text(json.dumps(self.raw_message()) + "\n" if opted_out else "")
            DEMUX.atomic_json(self.root / "leases" / f"{peer_id}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "lease_id": peer_id, "provider": "junie",
                "provider_session_id": "peer", "bus": str(other), "cursor": other.stat().st_size,
                "held": {}, "pending": []})
            with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                    mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                    mock.patch.object(DEMUX.time, "time", return_value=NOW + 5), \
                    mock.patch.object(DEMUX, "emit") as emitted:
                self.assertEqual(DEMUX.delivery_control_command(self.args(
                    channel="0", provider=None, session=None, lease=None,
                    revise=None, withdraw=row["message_id"], release=None)), 0)
            self.assertEqual([call.args[0] for call in publish.call_args_list], [self.bus])
            self.assertEqual([(item["channel"], item["reason"])
                              for item in emitted.call_args.args[0]["skipped"]], [("3", "not_held")])

    def test_broadcast_with_nobody_holding_refuses_without_publishing(self):
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        with mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX, "publish_reply_event") as publish:
            with self.assertRaisesRegex(ValueError, "no longer held by any"):
                DEMUX.delivery_control_command(self.args(channel="0", provider=None, session=None,
                    lease=None, revise=None, withdraw="a" * 24, release=None))
        publish.assert_not_called()

    def test_broadcast_controls_skip_a_dead_peer_without_a_copy(self):
        self.lease()
        self.bus.write_text(json.dumps(self.raw_message(
            message_id="a" * 24, hold_until=stamp(DEMUX.time.time() + 30))) + "\n")
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        other = self.home / "other.jsonl"
        other.touch()
        peer_id = DEMUX.lease_identifier("junie", "peer")
        DEMUX.atomic_json(self.root / "leases" / f"{peer_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": peer_id, "provider": "junie",
            "provider_session_id": "peer", "bus": str(other), "cursor": 0,
            "held": {}, "pending": []})
        DEMUX.write_channel_binding(self.root, "3", "peer", "junie", "peer", str(other))
        alive = {self.lease_id: 321}  # the peer's host restarted: binding and lease stay, no follower
        control = dict(channel="0", provider=None, session=None, lease=None,
                       revise=None, withdraw="a" * 24, release=None)
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", side_effect=lambda root, lease_id: alive.get(lease_id)), \
                mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.delivery_control_command(self.args(**control)), 0)
            self.assertEqual([call.args[0] for call in publish.call_args_list], [self.bus])
            published = emitted.call_args.args[0]
            # The held identity survives the roster walk: it names the row being withdrawn.
            self.assertEqual((published["identity"], publish.call_args.args[1]["identity"]), ("a" * 24, "a" * 24))
            self.assertEqual([r["channel"] for r in published["recipients"]], ["2"])
            self.assertEqual([(s["channel"], s["audience"], s["reason"]) for s in published["skipped"]],
                             [("3", "peer", "not_listening")])
        # Positive control: addressing the dead peer directly keeps the hard refusal.
        with mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=None):
            with self.assertRaisesRegex(ValueError, "no longer matches"):
                DEMUX.delivery_control_command(self.args(channel="3", provider="junie", session="peer",
                    lease=peer_id, bus=other, revise=None, withdraw="a" * 24, release=None))
            publish.assert_not_called()

    def test_broadcast_withdraw_reaches_an_original_owner_without_a_live_follower(self):
        self.lease()
        row = self.raw_message(message_id="a" * 24)
        self.bus.write_text(json.dumps(row) + "\n")
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        other = self.home / "other.jsonl"
        other.write_text(json.dumps(row) + "\n")
        peer_id = DEMUX.lease_identifier("junie", "peer")
        DEMUX.atomic_json(self.root / "leases" / f"{peer_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": peer_id, "provider": "junie",
            "provider_session_id": "peer", "bus": str(other), "cursor": 0,
            "held": {}, "pending": []})
        DEMUX.write_channel_binding(self.root, "3", "peer", "junie", "peer", str(other))
        # Only the first follower is running. Both owners still have this
        # exact message in their journals; stopping a reader cannot opt it out.
        with mock.patch.object(DEMUX, "live_follower_pid",
                side_effect=lambda root, lease: 321 if lease == self.lease_id else None), \
                mock.patch.object(DEMUX.time, "time", return_value=NOW + 5), \
                mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.delivery_control_command(self.args(
                channel="0", provider=None, session=None, lease=None,
                revise=None, withdraw=row["message_id"], release=None)), 0)
        self.assertEqual({call.args[0] for call in publish.call_args_list}, {self.bus, other})
        self.assertEqual(emitted.call_args.args[0]["skipped"], [])

    def test_voice_control_selects_the_capture_session_not_a_reused_numeric_label(self):
        self.lease()
        DEMUX.write_channel_binding(self.root, "2", "lena", self.provider, self.session, str(self.bus))
        rows = self.voice_journal_rows("agent-channel-2-first", "agent-channel-2-second")
        self.bus.write_text("".join(json.dumps(row) + "\n" for row in rows))
        args = self.args(revise=None, withdraw=None, release="agent-channel-2-second")
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX, "emit"):
            DEMUX.delivery_control_command(args)
            target = publish.call_args.args[1]
            self.assertEqual(target["utterance_id"], 42)
            self.assertEqual(target["session_id"], "agent-channel-2-second")
            self.assertEqual(target["identity"], "42")
            args.release = "42"
            with self.assertRaisesRegex(ValueError, "ambiguous"):
                DEMUX.delivery_control_command(args)
            self.assertEqual(publish.call_count, 1)

    def test_cli_rejects_mixed_or_negative_hold_commands_before_effects(self):
        for arguments in (("--withdraw", "id", "--status"),
                          ("--release", "id", "--channel", "0", "--session", "foreign"),
                          ("--send-text", "--hold-seconds", "-1")):
            with mock.patch.object(sys, "argv", [str(HELPER), *arguments]), \
                    mock.patch.object(DEMUX, "publish_reply_event") as publish, \
                    contextlib.redirect_stderr(io.StringIO()), \
                    self.assertRaises(SystemExit) as refused:
                DEMUX.main()
            self.assertEqual(refused.exception.code, 2)
            publish.assert_not_called()


if __name__ == "__main__":
    unittest.main()
