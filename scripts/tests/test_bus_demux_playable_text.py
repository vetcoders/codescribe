"""Text-only agent reply playback; private journals, mocked audio and credentials."""

import argparse
import base64
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "bus_playable_text_test", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


def fixture_publish(bus, event, *, bridge_root=None):
    bus.parent.mkdir(parents=True, exist_ok=True)
    raw = (json.dumps(event, ensure_ascii=False) + "\n").encode()
    with bus.open("ab") as handle:
        offset = handle.tell()
        handle.write(raw)
    metadata = bus.stat()
    manifest = {
        "schema": "codescribe.bus-generations.v1", "root": str(bus),
        "stream_id": "playable-text-fixture", "stream_inode": metadata.st_ino,
        "stream_dev": metadata.st_dev, "stream_birthtime": None,
        "segments": [], "pending": None,
        "active": {"id": "active", "path": str(bus), "start": 0,
                   "length": metadata.st_size, "dev": metadata.st_dev,
                   "ino": metadata.st_ino, "day": None, "compressed": False,
                   "sha256": None, "superseded": None},
    }
    Path(str(bus) + ".generations.json").write_text(json.dumps(manifest))
    return {"stream_id": manifest["stream_id"], "stream_dev": metadata.st_dev,
            "stream_inode": metadata.st_ino, "offset": offset, "length": len(raw)}


class PlayableTextTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.lease_id = DEMUX.lease_identifier("codex", "sender")
        self.args = argparse.Namespace(
            bridge_home=self.root, provider="codex", session="sender",
            bus=self.bus, bus_overridden=True, send="Odpowiedź bez głosu.", to="founder",
            playback_ticket="a" * 24,
        )
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "provider": "codex", "provider_session_id": "sender",
            "lease_id": self.lease_id, "name": "Olga", "bus": str(self.bus),
        })

    def publish_text(self):
        with patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish), \
                patch.object(DEMUX, "emit") as emit:
            self.assertEqual(DEMUX.send_peer_command(self.args), 0)
        identity = emit.call_args.args[0]["message_id"]
        self.args.play_reply = identity
        self.args.bus = self.root / "buses" / "channel-10.jsonl"
        return DEMUX.load_published_reply(self.args, identity)

    def test_text_send_has_playable_coordinates_after_sender_lease_is_gone(self):
        bus, reply = self.publish_text()
        self.assertNotIn("voice", reply)
        (self.root / "leases" / f"{self.lease_id}.json").unlink()
        self.assertEqual(DEMUX.load_published_reply(self.args, reply["reply_id"]), (bus, reply))

    def test_historical_voiceless_send_recovers_coordinates_without_a_live_lease(self):
        bus, reply = self.publish_text()
        source_path = self.root / "runtime" / "reply-sources" / f"{reply['reply_id']}.json"
        source_path.unlink()
        (self.root / "leases" / f"{self.lease_id}.json").unlink()
        self.assertEqual(DEMUX.load_published_reply(self.args, reply["reply_id"]), (bus, reply))
        self.assertTrue(source_path.is_file())

    def test_agent_authored_user_message_replays_through_its_sender_coordinates(self):
        identity = "d" * 24
        sender = {"name": "Olga", "provider": "codex", "provider_session_id": "sender",
                  "lease_id": self.lease_id}
        event = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                 "message_id": identity, "text": "Wiadomość agenta.", "sender": sender,
                 # These are the recipient, not the author of this message.
                 "provider": "claude-code", "provider_session_id": "recipient",
                 "lease_id": "e" * 32}
        fixture_publish(self.bus, event)
        self.args.play_reply = identity
        with patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as speak, \
                patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(DEMUX.play_reply_command(self.args), 0)
        self.assertEqual(speak.call_args.args[:3], (event["text"], "carina", 1.0))
        _, reply = DEMUX.load_published_reply(self.args, identity)
        self.assertEqual((reply["schema"], reply["reply_id"], reply["provider_session_id"]),
                         (DEMUX.AGENT_REPLY_SCHEMA, identity, "sender"))

    def test_founder_typed_message_never_becomes_an_agent_reply(self):
        identity = "d" * 24
        event = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                 "message_id": identity, "text": "Tekst Foundera.", "source": "typed",
                 "provider": "codex", "provider_session_id": "sender", "lease_id": self.lease_id}
        fixture_publish(self.bus, event)
        with self.assertRaises(ValueError):
            DEMUX.load_published_reply(self.args, identity)
        self.assertFalse((self.root / "runtime" / "reply-sources" / f"{identity}.json").exists())

    def test_historical_lookup_never_guesses_a_bus(self):
        _, reply = self.publish_text()
        (self.root / "runtime" / "reply-sources" / f"{reply['reply_id']}.json").unlink()
        self.args.bus_overridden = False
        with self.assertRaisesRegex(ValueError, "explicit bus"):
            DEMUX.load_published_reply(self.args, reply["reply_id"])

    def test_historical_chunked_reply_keeps_the_complete_canonical_byte_range(self):
        identity = "d" * 24
        event = {"schema": DEMUX.AGENT_REPLY_SCHEMA, "kind": "agent_reply", "reply_id": identity,
                 "text": "Wiadomość. " * 4000, "name": "Olga", "provider": "codex",
                 "provider_session_id": "sender", "lease_id": self.lease_id}
        payload = json.dumps(event).encode()
        blocks = [payload[start:start + 32768] for start in range(0, len(payload), 32768)]
        for part, block in enumerate(blocks):
            fixture_publish(self.bus, {
                "schema": "codescribe.bus-chunk.v1", "part": part, "parts": len(blocks),
                "length": len(payload), "id": hashlib.sha256(payload).hexdigest(),
                "event": {"schema": DEMUX.AGENT_REPLY_SCHEMA, "kind": "agent_reply"},
                "payload": base64.b64encode(block).decode(),
            })
        _, reply = DEMUX.load_published_reply(self.args, identity)
        self.assertEqual(reply, event)
        source = json.loads((self.root / "runtime" / "reply-sources" / f"{identity}.json").read_text())
        self.assertEqual(source["source"]["length"], self.bus.stat().st_size)

    def test_voiceless_play_resolves_sender_profile_and_then_system_defaults(self):
        reply = {"name": "Olga", "text": "Tekst"}
        self.assertEqual(
            {key: DEMUX.resolve_reply_voice(self.root, reply)[key]
             for key in ("voice", "speed", "tts_vendor")},
            {"voice": "carina", "speed": 1.0, "tts_vendor": "xai"})
        DEMUX.atomic_json(self.root / "voices.json", {
            "profiles": {"olga": {"voice": "nova", "speed": 1.2, "provider": "openai"}}})
        resolved = DEMUX.resolve_reply_voice(self.root, reply)
        self.assertEqual((resolved["voice"], resolved["speed"], resolved["tts_vendor"]),
                         ("nova", 1.2, "openai"))
        explicit = {**reply, "voice": "rex", "speed": 1.1, "tts_vendor": "xai"}
        self.assertEqual(DEMUX.resolve_reply_voice(self.root, explicit), explicit)

    def test_voiceless_play_synthesizes_once_and_reuses_audio_for_second_ticket(self):
        bus, reply = self.publish_text()
        with patch.object(DEMUX, "bus_path", return_value=bus), \
                patch.object(DEMUX, "_xai_speech_key", return_value="fixture-only") as key, \
                patch.object(DEMUX, "_tts_exchange", return_value=(b"\x01\x00", None, None)) as paid, \
                patch.object(DEMUX, "_play_pcm_24k", return_value=(True, None, None)) as player, \
                patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(DEMUX.play_reply_command(self.args), 0)
            self.args.playback_ticket = "b" * 24
            self.assertEqual(DEMUX.play_reply_command(self.args), 0)
        self.assertEqual(paid.call_count, 1)
        self.assertEqual(key.call_count, 1)
        self.assertEqual(player.call_count, 2)
        request = json.loads(paid.call_args.args[0].data)
        self.assertEqual((request["text"], request["voice_id"]), (reply["text"], "carina"))

    def test_broadcast_copy_coordinates_resolve_the_requested_recipient_bus(self):
        DEMUX.write_channel_binding(self.root, "1", "Olga", "codex", "sender", str(self.bus))
        for channel in ("2", "3"):
            DEMUX.write_channel_binding(self.root, channel, f"peer-{channel}", "codex",
                                        f"session-{channel}", str(self.root / f"bus-{channel}.jsonl"))
        self.args.to = "0"
        with patch.object(DEMUX, "publish_reply_event", side_effect=fixture_publish), \
                patch.object(DEMUX, "live_follower_pid", return_value=None), \
                patch.object(DEMUX, "emit") as emit:
            self.assertEqual(DEMUX.send_peer_command(self.args), 0)
        identity = emit.call_args.args[0]["message_id"]
        for channel in ("2", "3"):
            self.args.bus = self.root / f"bus-{channel}.jsonl"
            bus, reply = DEMUX.load_published_reply(self.args, identity)
            self.assertEqual(bus, self.args.bus)
            self.assertEqual(reply["peer_to"], f"peer-{channel}")


if __name__ == "__main__":
    unittest.main()
