"""Channel-zero policy separates durable receipt from waking a conversation."""
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
SPEC = importlib.util.spec_from_file_location("bus_broadcast_policy", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

RECEIPT = {"stream_id": "fixture", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}


class BroadcastPolicyTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()
        self.provider, self.session = "codex", "broadcast-session"
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

    def lease(self):
        lease = DEMUX.SessionLease(
            root=self.root, provider=self.provider, provider_session_id=self.session,
            name="lena", bus=self.bus, requested_id=None, ttl_seconds=120,
            follow_from_end=False, coalesce=True)
        self.addCleanup(lease.close)
        return lease

    def policy(self, value, provider=None, session=None, bus=None):
        provider, session, bus = provider or self.provider, session or self.session, bus or self.bus
        row = {"schema": DEMUX.BROADCAST_POLICY_SCHEMA, "provider": provider,
               "provider_session_id": session, "lease_id": DEMUX.lease_identifier(provider, session),
               "bus": str(bus), "policy": value, "emitted_at": DEMUX.utc_now()}
        path = DEMUX.broadcast_policy_path(self.root, provider, session, bus)
        DEMUX.atomic_json(path, row)
        return path

    def current_policy(self):
        return DEMUX.agent_broadcast_policy(self.root, self.provider, self.session, self.bus)

    def payload(self, lease, index=1, channel="0", **overrides):
        row = {"schema": DEMUX.EVENT_SCHEMA, "kind": "message", "source_event_id": f"{index:024x}",
               "audience": "lena", "channel": channel, "text": "Iwo Iwo Iwo Iwo Iwo",
               "state_change_allowed": True}
        row.update(overrides)
        lease.enrich(row)
        return row

    def cli(self, *arguments):
        output, error = io.StringIO(), io.StringIO()
        with mock.patch.object(sys, "argv", [str(HELPER), "--bridge-home", str(self.root), *arguments]), \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(error):
            result = DEMUX.main()
        return result, output.getvalue(), error.getvalue()

    def test_missing_file_defaults_to_on(self):
        self.assertEqual(self.current_policy(), "on")

    def test_policy_is_scoped_to_provider_session_and_bus(self):
        self.policy("off")
        self.assertEqual(self.current_policy(), "off")
        for provider, session, bus in (("junie", self.session, self.bus),
                                       (self.provider, "other-session", self.bus),
                                       (self.provider, self.session, self.home / "other-bus")):
            self.assertEqual(DEMUX.agent_broadcast_policy(self.root, provider, session, bus), "on")

    def test_invalid_receipts_fail_open_and_warn(self):
        path = self.policy("quiet")
        valid = json.loads(path.read_text())
        invalid = ["{broken", "[]", "null", "\udcff"]
        for field, value in (("schema", "wrong"), ("provider", "junie"),
                             ("provider_session_id", "foreign"), ("lease_id", "foreign"),
                             ("bus", str(self.home / "foreign")), ("policy", "muted"),
                             ("policy", ["quiet"])):
            invalid.append(json.dumps({**valid, field: value}))
        for raw in invalid:
            with self.subTest(raw=repr(raw)):
                path.write_bytes(raw.encode("utf-8", errors="surrogateescape"))
                error = io.StringIO()
                with contextlib.redirect_stderr(error):
                    self.assertEqual(self.current_policy(), "on")
                self.assertIn("broadcast policy; using on", error.getvalue())
        with mock.patch.object(DEMUX, "read_reply_json", side_effect=PermissionError), \
                contextlib.redirect_stderr(io.StringIO()) as error:
            self.assertEqual(self.current_policy(), "on")
        self.assertIn("using on", error.getvalue())

    def test_quiet_is_readable_and_acknowledgeable_but_not_waiter_ready(self):
        lease = self.lease()
        self.policy("quiet")
        quiet = self.payload(lease)
        self.assertTrue(lease.queue_delivery(quiet))
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [])
        with mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.read_pending_command(self.args()), 0)
        snapshot = emitted.call_args.args[0]
        self.assertEqual(snapshot["broadcast"], {"policy": "quiet"})
        self.assertEqual(snapshot["read_delivery_ids"], [quiet["delivery_id"]])
        self.assertEqual(snapshot["deliveries"][0]["text"].split(), ["Iwo"] * 5)
        with mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args(ack=[quiet["delivery_id"]])), 0)
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, quiet["delivery_id"], quiet))

    def test_off_writes_one_skip_receipt_without_mailbox_or_ack(self):
        lease = self.lease()
        self.policy("off")
        payload = self.payload(lease)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertFalse(lease.queue_delivery(payload))
            self.assertFalse(lease.queue_delivery(payload))
        self.assertEqual(lease.pending, {})
        self.assertEqual(output.getvalue(), "")
        events = DEMUX.follower_paths(self.root, self.lease_id)[1]
        rows = [json.loads(line) for line in events.read_text().splitlines()]
        self.assertEqual(len(rows), 1)
        self.assertEqual((rows[0]["kind"], rows[0]["origin_channel"], rows[0]["policy"]),
                         ("broadcast_skipped", "0", "off"))
        self.assertEqual(rows[0]["delivery_id"], payload["delivery_id"])
        self.assertNotIn("text", rows[0])
        self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, payload["delivery_id"]))
        # The overlay's receipt for this exact delivery and owner is the
        # per-delivery file, a copy of the recorded row.
        marker = DEMUX.broadcast_skip_path(self.root, self.lease_id, payload["delivery_id"])
        self.assertEqual(json.loads(marker.read_text()), rows[0])
        self.assertEqual(marker.stat().st_mode & 0o777, 0o600)

    def test_skip_marker_backfills_a_row_the_stream_already_holds(self):
        lease = self.lease()
        self.policy("off")
        payload = self.payload(lease)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(lease.queue_delivery(payload))
        marker = DEMUX.broadcast_skip_path(self.root, self.lease_id, payload["delivery_id"])
        recorded = json.loads(marker.read_text())
        marker.unlink()
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(lease.queue_delivery(payload))
        events = DEMUX.follower_paths(self.root, self.lease_id)[1]
        self.assertEqual(len(events.read_text().splitlines()), 1, "no second history row")
        self.assertEqual(json.loads(marker.read_text()), recorded)

    def test_all_producer_addresses_are_broadcast_but_direct_is_not(self):
        lease = self.lease()
        self.policy("off")
        for index, address in enumerate((
            {"origin_channel": "0", "channel": "2", "audience": "lena"},
            {"channel": "0", "peer_to": "lena"},
            {"audience": "*"},
        ), 1):
            with self.subTest(address=address):
                row = self.payload(lease, index, channel="2", **{k: v for k, v in address.items() if k != "channel"})
                row.update(address)
                self.assertFalse(lease.queue_delivery(row))
        direct = self.payload(lease, 4, channel="2", peer_to="lena")
        self.assertTrue(lease.queue_delivery(direct))
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [direct])

    def test_cli_validates_exact_lease_and_can_infer_its_bus(self):
        self.lease()
        owner = ["--provider", self.provider, "--session", self.session]
        for policy in ("quiet", "off", "on"):
            code, output, error = self.cli("--broadcast", policy, *owner)
            self.assertEqual(code, 0, error)
            row = json.loads(output)
            self.assertEqual(row["broadcast"], {"policy": policy})
            self.assertEqual(row["bus"], str(self.bus))
            self.assertEqual(self.current_policy(), policy)
        path = DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus)
        before = path.read_bytes()
        code, output, _ = self.cli("--broadcast", "off", *owner, "--bus", str(self.home / "foreign"))
        self.assertEqual((code, output), (3, ""))
        lease_path = self.root / "leases" / f"{self.lease_id}.json"
        foreign = json.loads(lease_path.read_text())
        foreign["provider_session_id"] = "foreign-session"
        DEMUX.atomic_json(lease_path, foreign)
        code, output, error = self.cli("--broadcast", "off", *owner)
        self.assertEqual((code, output), (3, ""))
        self.assertIn("does not belong", error)
        self.assertEqual(path.read_bytes(), before)

    def test_cli_rejects_mixed_commands_before_any_effect(self):
        self.lease()
        for extra in (["--status"], ["--attach"], ["--read-pending"], ["--mute-agent"], ["--watch"]):
            with self.subTest(extra=extra), self.assertRaises(SystemExit) as refused, \
                    contextlib.redirect_stderr(io.StringIO()):
                self.cli("--broadcast", "quiet", "--provider", self.provider,
                         "--session", self.session, *extra)
            self.assertEqual(refused.exception.code, 2)
        self.assertEqual(self.current_policy(), "on")

    def test_attach_and_status_project_the_same_receipt(self):
        lease = self.lease()
        self.policy("off")
        self.assertEqual(lease.attach_receipt()["broadcast"], {"policy": "off"})
        with mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.status_command(self.args()), 0)
        self.assertEqual(emitted.call_args.args[0]["broadcast"], {"policy": "off"})

    def run_follower(self, events, *, switch=None):
        args = self.args(name="lena", all=False, become=False, follow=True, from_start=True,
                         coalesce=True, lease=None, on_seal="fixture-hook", wakeup="codex-queue",
                         drafts=True, debug=False, once=False, interval=0.01,
                         follower_events=DEMUX.follower_paths(self.root, self.lease_id)[1],
                         follower_channel="2")
        args.follower_events.parent.mkdir(parents=True, exist_ok=True)
        index = 0
        def read(_path, _offset):
            nonlocal index
            if index == len(events):
                raise KeyboardInterrupt
            if switch is not None:
                switch(index)
            row = events[index]
            index += 1
            return [(json.dumps(row), index)], index
        output = io.StringIO()
        with mock.patch.object(DEMUX, "iter_new_lines", side_effect=read), \
                mock.patch.object(DEMUX, "BusEventTrigger"), \
                mock.patch.object(DEMUX, "NativeQueueWakeup") as native, \
                mock.patch.object(DEMUX, "fire_seal_hook") as hook, \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(DEMUX.run(args), 130)
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        return state, output.getvalue(), native.return_value.enqueue.call_args_list, hook.call_args_list

    def voice(self, index, audience="*"):
        return {"schema": DEMUX.CLEAN_SCHEMA, "status": DEMUX.SEALED,
                "session_id": "broadcast-take", "utterance_id": f"u-{index}",
                "sequence": index, "audience": audience, "text": f"message-{index}",
                "emitted_at": "2026-10-15T18:00:00Z"}

    def test_quiet_follower_has_no_output_native_or_hook_bell(self):
        # The direct half moved to test_bus_steer_global.py: since Steer went
        # global (Maciej, 2026-10-17) a quiet direct is mail too, not a bell.
        self.policy("quiet")
        state, output, native, hooks = self.run_follower([self.voice(1), self.voice(2, "lena")])
        self.assertEqual(len(state["pending"]), 2)
        self.assertNotIn("message-1", output)
        self.assertNotIn("message-2", output)
        self.assertEqual((native, hooks), ([], []))
        rows = [json.loads(line) for line in DEMUX.follower_paths(self.root, self.lease_id)[1].read_text().splitlines()]
        self.assertEqual([row["text"] for row in rows if "text" in row], [])

    def test_policy_switches_between_deliveries_in_the_same_follower(self):
        state, output, native, hooks = self.run_follower(
            [self.voice(1), self.voice(2), self.voice(3)],
            switch=lambda index: self.policy(("quiet", "on", "off")[index]))
        self.assertEqual([row["text"] for row in state["pending"]], ["message-1", "message-2"])
        self.assertNotIn("message-1", output)
        self.assertIn("message-2", output)
        self.assertNotIn("message-3", output)
        self.assertEqual(len(native), 1)
        self.assertEqual(len(hooks), 1)
        rows = [json.loads(line) for line in DEMUX.follower_paths(self.root, self.lease_id)[1].read_text().splitlines()]
        self.assertEqual(sum(row.get("kind") == "broadcast_skipped" for row in rows), 1)

    def test_waiter_does_not_complete_for_quiet_pending(self):
        lease = self.lease()
        self.policy("quiet")
        lease.queue_delivery(self.payload(lease))
        with mock.patch.object(DEMUX, "BusEventTrigger"), \
                mock.patch.object(DEMUX.time, "monotonic", side_effect=[0, 1]), \
                mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.wait_for_pending_command(self.args(max_wait=0.5, interval=0.01)), 0)
        self.assertEqual(emitted.call_args.args[0]["kind"], "watch_timeout")

    # test_watch_suppresses_broadcast_rows_but_not_direct_rows left with the
    # old contract: test_bus_steer_global.py pins watch over both channels
    # now that a quiet direct row is suppressed exactly like a broadcast one.

    def test_restarting_a_quiet_follower_does_not_ring_for_its_pending_broadcast(self):
        self.policy("quiet")
        self.run_follower([self.voice(1)])
        state, output, native, hooks = self.run_follower([])
        self.assertEqual(len(state["pending"]), 1)
        self.assertNotIn("message-1", output)
        self.assertEqual((native, hooks), ([], []))

    def test_native_queue_rechecks_policy_even_for_an_explicit_retry(self):
        lease = self.lease()
        row = self.payload(lease)
        self.assertTrue(lease.queue_delivery(row))
        native = DEMUX.NativeQueueWakeup(self.root, self.session, "2")
        self.addCleanup(lambda: native.close(wait=True))
        for policy in ("quiet", "off"):
            self.policy(policy)
            with mock.patch("subprocess.run") as submit:
                native._deliver(row, retry=True)
            submit.assert_not_called()
        self.assertFalse((self.root / "wakeups" / self.lease_id / f"{row['delivery_id']}.json").exists())

    def test_policy_is_read_when_a_delivery_lands_not_at_exit(self):
        # The follower flushes buffered human drafts after every batch, so a
        # draft that landed under "on" is already on stdout before a later
        # switch to quiet; the seal that lands after the switch stays quiet.
        # Policy is decided per delivery, never replayed against old rows.
        draft = {**self.voice(1), "status": "utterance_draft"}
        self.policy("on")
        state, output, _native, _hooks = self.run_follower(
            [draft, self.voice(2)], switch=lambda index: self.policy(("on", "quiet")[index]))
        self.assertEqual(len(state["pending"]), 2)
        self.assertIn("message-1", output)
        self.assertNotIn("message-2", output)

    def test_publisher_receipts_include_each_policy_without_suppressing_publication(self):
        lease = self.lease()
        DEMUX.write_channel_binding(self.root, "1", "lena", self.provider, self.session, str(self.bus))
        for digit, policy in (("2", "quiet"), ("3", "off")):
            session = f"peer-{digit}"
            DEMUX.write_channel_binding(self.root, digit, session, "junie", session, str(self.bus))
            DEMUX.atomic_json(self.root / "leases" / f"{DEMUX.lease_identifier('junie', session)}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "lease_id": DEMUX.lease_identifier("junie", session),
                "provider": "junie", "provider_session_id": session, "bus": str(self.bus), "name": session})
            self.policy(policy, provider="junie", session=session)
        with mock.patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                mock.patch.object(DEMUX, "live_follower_pid", return_value=321), \
                mock.patch.object(DEMUX, "emit") as emitted, \
                mock.patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO(b"typed broadcast"))):
            self.assertEqual(DEMUX.send_text_command(self.args(channel="0", attach_file=[])), 0)
            self.assertEqual(publish.call_count, 3)
            self.assertEqual([row["broadcast"] for row in emitted.call_args.args[0]["recipients"]],
                             ["on", "quiet", "off"])
            self.assertTrue(all(call.args[1]["origin_channel"] == "0" for call in publish.call_args_list))
            publish.reset_mock()
            self.assertEqual(DEMUX.send_peer_command(self.args(send="peer broadcast", to="0")), 0)
            self.assertEqual(publish.call_count, 2)
            self.assertEqual([row["broadcast"] for row in emitted.call_args.args[0]["recipients"]],
                             ["quiet", "off"])
        self.assertEqual(lease.pending, {})


if __name__ == "__main__":
    unittest.main()
