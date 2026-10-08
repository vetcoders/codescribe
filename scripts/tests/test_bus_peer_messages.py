"""Agent-to-agent text messages: explicit peer routing, no echo, no authority."""
import argparse
from contextlib import closing
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("bus_peer_messages", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

RECEIPT = {"stream_id": "fixture", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}


def sender_lease(root: Path, provider: str, session: str, name: str) -> str:
    lease_id = DEMUX.lease_identifier(provider, session)
    DEMUX.atomic_json(root / "leases" / f"{lease_id}.json", {
        "schema": DEMUX.LEASE_SCHEMA, "provider": provider, "provider_session_id": session,
        "lease_id": lease_id, "name": name, "bus": str(root / "bus-1.jsonl")})
    return lease_id


def send_args(root: Path, text: str, to: str, provider: str = "claude-code",
              session: str = "sender-session") -> argparse.Namespace:
    return argparse.Namespace(bridge_home=root, provider=provider, session=session,
                              send=text, to=to)


class PeerSendTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        DEMUX.write_channel_binding(self.root, "1", "vagabond", "claude-code",
                                    "sender-session", str(self.root / "bus-1.jsonl"))
        DEMUX.write_channel_binding(self.root, "2", "lena", "codex",
                                    "lena-session", str(self.root / "bus-2.jsonl"))
        self.lease_id = sender_lease(self.root, "claude-code", "sender-session", "vagabond")

    def test_direct_message_targets_only_the_named_peer_bus(self):
        with patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
             patch.object(DEMUX, "live_follower_pid", return_value=321), \
             patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.send_peer_command(send_args(self.root, "cześć Lena", "LENA")), 0)
        self.assertEqual(publish.call_count, 1)
        bus, event = publish.call_args.args
        self.assertEqual(bus, (self.root / "bus-2.jsonl").resolve())
        self.assertEqual(event["schema"], DEMUX.AGENT_REPLY_SCHEMA)
        self.assertEqual(event["peer_to"], "lena")
        self.assertEqual(event["channel"], "2")
        self.assertEqual(event["message_id"], event["reply_id"])
        self.assertEqual(event["sender"]["name"], "vagabond")
        self.assertEqual(event["sender"]["channel"], "1")
        self.assertEqual(event["sender"]["lease_id"], self.lease_id)
        self.assertIs(event["spoken"], False)
        self.assertEqual(event["association"], "unsolicited")
        self.assertIsNone(event["delivery_id"])

    def test_broadcast_to_zero_reaches_every_peer_and_never_the_sender(self):
        DEMUX.write_channel_binding(self.root, "3", "astra", "codex",
                                    "astra-session", str(self.root / "bus-3.jsonl"))
        with patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
             patch.object(DEMUX, "live_follower_pid", return_value=None), \
             patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.send_peer_command(send_args(self.root, "fala do wszystkich", "0")), 0)
        self.assertEqual(publish.call_count, 2)
        events = [call.args[1] for call in publish.call_args_list]
        self.assertEqual(sorted(event["peer_to"] for event in events), ["astra", "lena"])
        self.assertEqual({event["channel"] for event in events}, {"0"})
        self.assertEqual(len({event["message_id"] for event in events}), 1)
        self.assertNotIn("vagabond", {event["peer_to"] for event in events})

    def test_sender_without_attached_name_is_refused(self):
        lease_id = DEMUX.lease_identifier("codex", "anonymous")
        DEMUX.atomic_json(self.root / "leases" / f"{lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "provider": "codex", "provider_session_id": "anonymous",
            "lease_id": lease_id, "bus": str(self.root / "bus-9.jsonl")})
        with patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish:
            with self.assertRaises(ValueError):
                DEMUX.send_peer_command(send_args(self.root, "hej", "lena",
                                                  provider="codex", session="anonymous"))
        self.assertEqual(publish.call_count, 0)


class PeerAdmissionTests(unittest.TestCase):
    def peer_event(self, **overrides):
        identity = "ab" * 12
        event = {"schema": DEMUX.AGENT_REPLY_SCHEMA, "kind": "agent_reply",
                 "reply_id": identity, "message_id": identity,
                 "source_event_id": identity, "name": "vagabond",
                 "provider": "claude-code", "provider_session_id": "sender-session",
                 "lease_id": "x" * 32, "text": "do roboty", "spoken": False,
                 "association": "unsolicited", "delivery_id": None,
                 "peer_to": "lena", "channel": "0",
                 "sender": {"name": "vagabond", "provider": "claude-code",
                            "provider_session_id": "sender-session", "lease_id": "x" * 32,
                            "channel": "1"},
                 "recipients": []}
        event.update(overrides)
        return event

    def test_peer_message_is_admitted_without_founder_authority(self):
        events = DEMUX.normalized_revision_events(json.dumps(self.peer_event()),
                                                  DEMUX.EvidenceNormalizer())
        self.assertEqual(len(events), 1)
        payload = DEMUX.consider(events[0], name="lena", hear_all=False, drafts=False, debug=False)
        self.assertEqual(payload["kind"], "message")
        self.assertIs(payload["state_change_allowed"], False)
        self.assertEqual(payload["producer_schema"], DEMUX.AGENT_REPLY_SCHEMA)
        self.assertEqual(payload["channel"], "0")
        self.assertEqual(payload["sender"]["name"], "vagabond")

    def test_peer_message_for_another_name_stays_out_of_this_mailbox(self):
        payload = DEMUX.consider(self.peer_event(), name="astra", hear_all=False,
                                 drafts=False, debug=False)
        self.assertIsNone(payload)

    def test_ordinary_spoken_reply_never_echoes_into_any_mailbox(self):
        plain = self.peer_event()
        del plain["peer_to"]
        plain["audience"] = "lena"
        for name in ("lena", "vagabond"):
            self.assertIsNone(DEMUX.consider(plain, name=name, hear_all=False,
                                             drafts=False, debug=False))

    def test_malformed_sender_is_refused(self):
        broken = self.peer_event(sender={"name": "vagabond"})
        self.assertIsNone(DEMUX.consider(broken, name="lena", hear_all=False,
                                         drafts=False, debug=False))

    def test_two_peer_messages_queue_as_distinct_deliveries_and_dedupe(self):
        with tempfile.TemporaryDirectory() as directory:
            with closing(DEMUX.SessionLease(root=Path(directory), provider="codex",
                    provider_session_id="lena-session", name="lena",
                    bus=Path(directory) / "bus", requested_id=None, ttl_seconds=30,
                    follow_from_end=False, coalesce=True)) as lease:
                ids = []
                for index in range(2):
                    event = self.peer_event(reply_id=f"{index + 1:024x}",
                                            message_id=f"{index + 1:024x}",
                                            source_event_id=f"{index + 1:024x}")
                    payload = DEMUX.consider(event, name="lena", hear_all=False,
                                             drafts=False, debug=False)
                    lease.enrich(payload)
                    self.assertTrue(lease.queue_delivery(payload))
                    self.assertFalse(lease.queue_delivery(payload))
                    ids.append(payload["delivery_id"])
                self.assertEqual(len(set(ids)), 2)
                self.assertEqual(len(lease.pending), 2)


if __name__ == "__main__":
    unittest.main()
