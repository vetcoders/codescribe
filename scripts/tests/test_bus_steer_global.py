"""Steer governs every delivery, not only channel-zero broadcasts.

Maciej, 2026-10-17 01:33Z-01:39Z (channel 0): the agent row's policy becomes
state checkboxes, "Channel 0" and "Steer (otherwise queued)", and the queued
side is a soft queue ("nie no - miękka kolejka"), never a refusal. Today the
helper asks the policy only for broadcasts (`delivery_broadcast_policy`
returns "on" for anything else), so a direct or peer-to-peer delivery always
wakes the conversation and there is no "read it at my next turn" rung for
them. These tests pin the new contract: under quiet a direct is mail, not a
wake; leaving channel zero never touches directs.

Supersedes, in test_bus_broadcast_policy.py, the direct-still-wakes halves of
`test_quiet_follower_has_no_output_native_or_hook_bell_but_direct_does` and
`test_watch_suppresses_broadcast_rows_but_not_direct_rows`.
"""
import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
import unittest.mock as mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_steer_global", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class SteerGlobalTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()
        self.provider, self.session = "codex", "steer-session"
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

    def policy(self, value):
        row = {"schema": DEMUX.BROADCAST_POLICY_SCHEMA, "provider": self.provider,
               "provider_session_id": self.session, "lease_id": self.lease_id,
               "bus": str(self.bus), "policy": value, "emitted_at": DEMUX.utc_now()}
        DEMUX.atomic_json(DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus), row)

    def direct(self, lease, index=1, **overrides):
        # A named direct on the agent's own digit: never a broadcast, see
        # is_broadcast_delivery and test_all_producer_addresses_are_broadcast_but_direct_is_not.
        row = {"schema": DEMUX.EVENT_SCHEMA, "kind": "message", "source_event_id": f"{index:024x}",
               "audience": "lena", "channel": "2", "text": "Iwo Iwo Iwo Iwo Iwo",
               "state_change_allowed": True}
        row.update(overrides)
        lease.enrich(row)
        return row

    def peer(self, lease, index=2):
        return self.direct(lease, index, peer_to="lena", text="peer peer peer")

    def events_rows(self):
        events = DEMUX.follower_paths(self.root, self.lease_id)[1]
        if not events.exists():
            return []
        return [json.loads(line) for line in events.read_text().splitlines()]

    def run_follower(self, events):
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

    def voice(self, index, audience):
        return {"schema": DEMUX.CLEAN_SCHEMA, "status": DEMUX.SEALED,
                "session_id": "steer-take", "utterance_id": f"u-{index}",
                "sequence": index, "audience": audience, "text": f"message-{index}",
                "emitted_at": "2026-10-17T01:40:00Z"}

    # --- quiet: directs and peers are mail, not wakes ---------------------

    def test_quiet_queues_a_direct_as_readable_mail_without_waking_the_waiter(self):
        lease = self.lease()
        self.policy("quiet")
        direct = self.direct(lease)
        self.assertTrue(lease.queue_delivery(direct), "quiet keeps the mail; only off skips, and only zero")
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [],
                         "a direct under quiet waits for the agent's own turn")
        with mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.read_pending_command(self.args()), 0)
        snapshot = emitted.call_args.args[0]
        self.assertEqual(snapshot["read_delivery_ids"], [direct["delivery_id"]])
        self.assertEqual(snapshot["broadcast"], {"policy": "quiet", "membership": "on", "quiet": True})
        with mock.patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args(ack=[direct["delivery_id"]])), 0)
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, direct["delivery_id"], direct))

    def test_quiet_queues_a_peer_message_the_same_way(self):
        lease = self.lease()
        self.policy("quiet")
        peer = self.peer(lease)
        self.assertTrue(lease.queue_delivery(peer))
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [])
        self.assertEqual([row["delivery_id"] for row in DEMUX.unread_pending(self.args(), include_quiet=True)[1]],
                         [peer["delivery_id"]])
        self.assertEqual(self.events_rows(), [], "no skip receipt: quiet is a queue, not a refusal")

    def test_quiet_follower_rings_no_bell_for_a_direct_take(self):
        self.policy("quiet")
        state, output, native, hooks = self.run_follower([self.voice(1, "lena")])
        self.assertEqual([row["text"] for row in state["pending"]], ["message-1"], "the take is kept")
        self.assertNotIn("message-1", output, "no stdout bell for the provider")
        self.assertEqual((native, hooks), ([], []), "no native enqueue, no seal hook")
        self.assertEqual([row["text"] for row in self.events_rows() if "text" in row], [])

    def test_waiter_does_not_complete_for_a_quiet_direct(self):
        lease = self.lease()
        self.policy("quiet")
        lease.queue_delivery(self.direct(lease))
        with mock.patch.object(DEMUX, "BusEventTrigger"), \
                mock.patch.object(DEMUX.time, "monotonic", side_effect=[0, 1]), \
                mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.wait_for_pending_command(self.args(max_wait=0.5, interval=0.01)), 0)
        self.assertEqual(emitted.call_args.args[0]["kind"], "watch_timeout")

    def test_watch_suppresses_quiet_direct_rows_like_broadcast_rows(self):
        lease = self.lease()
        events = DEMUX.follower_paths(self.root, self.lease_id)[1]
        events.parent.mkdir(parents=True, exist_ok=True)
        for index, channel in enumerate(("0", "2"), 1):
            row = self.direct(lease, index, channel=channel)
            lease.queue_delivery(row)
            DEMUX.emit_follower(row, events, "2", human_output=False)
        self.policy("quiet")
        with mock.patch.object(DEMUX, "emit") as emitted:
            self.assertEqual(DEMUX.watch_command(self.args(
                from_file=None, once=True, human=False, full=False)), 0)
        self.assertEqual(emitted.call_count, 0, "nothing rings while Steer is off")

    # --- off: leaving channel zero never touches directs -----------------

    def test_leaving_channel_zero_keeps_directs_and_peers_as_mail(self):
        lease = self.lease()
        self.policy("off")
        direct, peer = self.direct(lease, 1), self.peer(lease, 2)
        self.assertTrue(lease.queue_delivery(direct))
        self.assertTrue(lease.queue_delivery(peer))
        self.assertEqual([row.get("kind") for row in self.events_rows()], [],
                         "no broadcast_skipped receipt for a direct or a peer")
        ids = [row["delivery_id"] for row in DEMUX.unread_pending(self.args(), include_quiet=True)[1]]
        self.assertEqual(ids, [direct["delivery_id"], peer["delivery_id"]])


if __name__ == "__main__":
    unittest.main()
