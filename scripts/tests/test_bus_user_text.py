"""Written messages use immutable recipients, canonical publication and native wakeup."""
import argparse
from contextlib import closing
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("bus_user_text", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

class UserTextTests(unittest.TestCase):
    def test_five_equal_messages_keep_distinct_delivery_ids_without_pcm(self):
        with tempfile.TemporaryDirectory() as directory:
            with closing(DEMUX.SessionLease(root=Path(directory), provider="codex", provider_session_id="agent-a",
                    name="lena", bus=Path(directory) / "bus", requested_id=None, ttl_seconds=30,
                    follow_from_end=False, coalesce=True)) as lease:
                ids = []
                for index in range(5):
                    identity = f"{index + 1:024x}"
                    event = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                             "source": "typed", "message_id": identity, "source_event_id": identity,
                             "audience": "lena", "text": "Iwo"}
                    events = DEMUX.normalized_revision_events(json.dumps(event), DEMUX.EvidenceNormalizer())
                    self.assertEqual(len(events), 1)
                    payload = DEMUX.consider(events[0], name="lena", hear_all=False, drafts=False, debug=False)
                    self.assertEqual(payload["kind"], "message")
                    self.assertNotIn("status", payload)
                    self.assertNotIn("wav", payload)
                    lease.enrich(payload)
                    self.assertTrue(lease.queue_delivery(payload))
                    self.assertFalse(lease.queue_delivery(payload))
                    ids.append(payload["delivery_id"])
                self.assertEqual(len(set(ids)), 5)
                self.assertEqual(len(lease.pending), 5)

    def test_publish_holds_binding_identity_and_refuses_rebound_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            bus = root / "bus.jsonl"
            lease_id = DEMUX.lease_identifier("codex", "agent-a")
            DEMUX.write_channel_binding(root, "2", "lena", "codex", "agent-a", str(bus))
            DEMUX.atomic_json(root / "leases" / f"{lease_id}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "provider": "codex", "provider_session_id": "agent-a",
                "lease_id": lease_id, "bus": str(bus)})
            args = argparse.Namespace(bridge_home=root, channel="2", bus=bus, lease=lease_id,
                                      provider="codex", session="agent-a")
            with patch.object(DEMUX, "live_follower_pid", return_value=123), \
                 patch.object(DEMUX, "publish_reply_event", return_value={"stream_id":"fixture", "stream_dev":1, "stream_inode":2, "offset":0, "length":12}) as publish, \
                 patch.object(DEMUX, "emit"), patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO("Iwo".encode()))):
                self.assertEqual(DEMUX.send_text_command(args), 0)
                event = publish.call_args.args[1]
                self.assertEqual(event["recipients"][0]["lease_id"], lease_id)
                self.assertEqual(event["source"], "typed")
                self.assertNotIn("sample_start", event)
                self.assertNotIn("status", event)
                state = DEMUX.read_json(root / DEMUX.AUDIENCE_BINDING_FILENAME)
                state["bindings"]["2"]["provider_session_id"] = "foreign"
                DEMUX.atomic_json(root / DEMUX.AUDIENCE_BINDING_FILENAME, state)
                with self.assertRaises(ValueError): DEMUX.send_text_command(args)
                self.assertEqual(publish.call_count, 1)

if __name__ == "__main__": unittest.main()
