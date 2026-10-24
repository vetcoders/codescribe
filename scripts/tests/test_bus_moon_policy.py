"""The moon and channel-zero membership are two independent axes.

Maciej, 2026-10-23 (take a9351d39): "Do not interrupt działa tylko dla
broadcastu, co jest w ogóle bez sensu" — an agent in a meeting with the
Founder kept being woken by messages from other channels, and the quiet
checkbox could only be ticked while the agent sat on channel 0. From here on
the moon ("tryb księżyc") says whether anything may wake the agent — every
channel, the Founder's directs and peers included — and the antenna says only
whether the agent is on channel 0. Any combination is legal, off channel 0
with the moon lit among them. A receipt written before the split spelled the
lit moon as policy "quiet"; it still reads as on channel 0 with the moon lit.
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
from unittest import mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_moon_policy", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class MoonPolicyTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name).resolve()
        self.root = self.home / "bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()
        self.provider, self.session = "codex", "moon-session"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        metadata = mock.patch.object(DEMUX, "NativeProviderModelReader", return_value=None)
        metadata.start()
        self.addCleanup(metadata.stop)

    def args(self, **overrides):
        values = dict(bridge_home=self.root, bus=self.bus, bus_overridden=True,
                      provider=self.provider, session=self.session, broadcast=None, quiet=None,
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

    def receipt(self, policy, quiet=None):
        row = {"schema": DEMUX.BROADCAST_POLICY_SCHEMA, "provider": self.provider,
               "provider_session_id": self.session, "lease_id": self.lease_id,
               "bus": str(self.bus), "policy": policy, "emitted_at": DEMUX.utc_now()}
        if quiet is not None:
            row["quiet"] = quiet
        DEMUX.atomic_json(DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus), row)

    def axes(self):
        return DEMUX.agent_bus_policy(self.root, self.provider, self.session, self.bus)

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

    # --- reading -----------------------------------------------------------

    def test_missing_receipt_is_on_channel_zero_with_the_moon_dark(self):
        self.assertEqual(self.axes(), ("on", False))

    def test_legacy_quiet_receipt_reads_as_member_with_the_moon_lit(self):
        self.receipt("quiet")
        self.assertEqual(self.axes(), ("on", True))
        self.assertEqual(DEMUX.agent_broadcast_policy(self.root, self.provider, self.session, self.bus), "quiet")

    def test_off_with_the_moon_lit_skips_broadcasts_and_queues_directs(self):
        lease = self.lease()
        self.receipt("off", quiet=True)
        self.assertEqual(self.axes(), ("off", True))
        broadcast = self.payload(lease, 1, channel="0")
        direct = self.payload(lease, 2, channel="2", origin_channel="2", peer_to="lena")
        self.assertEqual(DEMUX.delivery_broadcast_policy(self.root, broadcast), "off",
                         "not a member: a broadcast is skipped whatever the moon")
        self.assertEqual(DEMUX.delivery_broadcast_policy(self.root, direct), "quiet",
                         "the moon holds a direct even off channel 0 — the former wire could not say this")
        self.assertFalse(lease.queue_delivery(broadcast))
        self.assertTrue(lease.queue_delivery(direct))
        self.assertEqual(DEMUX.unread_pending(self.args())[1], [], "nothing is waiter-ready under the moon")
        self.assertEqual([row["delivery_id"] for row in DEMUX.unread_pending(self.args(), include_quiet=True)[1]],
                         [direct["delivery_id"]])

    def test_on_with_the_moon_dark_wakes_for_both(self):
        lease = self.lease()
        self.receipt("on", quiet=False)
        for channel in ("0", "2"):
            self.assertEqual(DEMUX.delivery_broadcast_policy(self.root, self.payload(lease, channel=channel)), "on")

    def test_a_non_boolean_moon_fails_open(self):
        self.receipt("on", quiet="yes")
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.axes(), ("on", False))

    # --- writing -------------------------------------------------------------

    def test_each_flag_moves_its_own_axis_and_keeps_the_other(self):
        self.lease()
        owner = ["--provider", self.provider, "--session", self.session]
        code, output, error = self.cli("--quiet", "on", *owner)
        self.assertEqual(code, 0, error)
        self.assertEqual(json.loads(output)["broadcast"], {"policy": "quiet", "membership": "on", "quiet": True})
        self.assertEqual(self.axes(), ("on", True))
        code, output, error = self.cli("--broadcast", "off", "--quiet", "on", *owner)
        self.assertEqual(code, 0, error)
        self.assertEqual(json.loads(output)["broadcast"], {"policy": "off", "membership": "off", "quiet": True},
                         "leaving channel 0 keeps the moon lit")
        self.assertEqual(self.axes(), ("off", True))
        code, output, error = self.cli("--quiet", "off", *owner)
        self.assertEqual(code, 0, error)
        self.assertEqual(self.axes(), ("off", False), "clearing the moon keeps the agent off channel 0")
        row = json.loads(DEMUX.broadcast_policy_path(self.root, self.provider, self.session, self.bus).read_text())
        self.assertEqual((row["policy"], row["quiet"]), ("off", False))

    def test_broadcast_alone_keeps_its_one_knob_meaning(self):
        # An agent that knows only `--broadcast on|quiet|off` (skills written
        # before the split) still gets exactly what it asked for.
        self.lease()
        owner = ["--provider", self.provider, "--session", self.session]
        self.cli("--quiet", "on", *owner)
        self.cli("--broadcast", "on", *owner)
        self.assertEqual(self.axes(), ("on", False), "plain on wakes again, as it always did")
        self.cli("--broadcast", "quiet", *owner)
        self.assertEqual(self.axes(), ("on", True))
        self.cli("--broadcast", "off", *owner)
        self.assertEqual(self.axes(), ("off", False))

    def test_both_flags_in_one_command_write_one_receipt(self):
        self.lease()
        code, output, error = self.cli("--broadcast", "on", "--quiet", "on",
                                       "--provider", self.provider, "--session", self.session)
        self.assertEqual(code, 0, error)
        self.assertEqual(self.axes(), ("on", True))

    def test_legacy_quiet_spelling_beside_the_moon_flag_only_names_membership(self):
        self.lease()
        owner = ["--provider", self.provider, "--session", self.session]
        self.cli("--broadcast", "off", *owner)
        code, _, error = self.cli("--broadcast", "quiet", "--quiet", "off", *owner)
        self.assertEqual(code, 0, error)
        self.assertEqual(self.axes(), ("on", False), "--quiet is the moon's only word when both are given")

    def test_quiet_flag_alone_needs_the_owner(self):
        with self.assertRaises(SystemExit) as refused, contextlib.redirect_stderr(io.StringIO()):
            self.cli("--quiet", "on")
        self.assertEqual(refused.exception.code, 2)


if __name__ == "__main__":
    unittest.main()
