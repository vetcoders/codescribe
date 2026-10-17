"""Writer's quotes outside code in agent replies and peer messages (Founder, 2026-10-14)."""
import argparse
import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bus_typographic_quotes", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

RECEIPT = {"stream_id": "fixture", "stream_dev": 1, "stream_inode": 2, "offset": 0, "length": 12}


class TypographicQuotesTests(unittest.TestCase):
    def test_polish_pairs_outside_code_and_straight_inside(self):
        self.assertEqual(
            DEMUX.typographic_quotes('Powiedz "cześć" i wpisz `echo "ok"` potem "dalej".'),
            "Powiedz „cześć” i wpisz `echo \"ok\"` potem „dalej”.")

    def test_english_without_diacritics_fenced_blocks_odd_quotes_and_apostrophes(self):
        self.assertEqual(DEMUX.typographic_quotes('say "hi"'), "say “hi”")
        text = 'Tytuł "A"\n```\nprint("x")\n```\nTo "B" i take\'u "C'
        self.assertEqual(
            DEMUX.typographic_quotes(text),
            'Tytuł „A”\n```\nprint("x")\n```\nTo „B” i take\'u "C')
        self.assertEqual(DEMUX.typographic_quotes('a "b" `c "d"'), 'a “b” `c "d"')
        self.assertEqual(DEMUX.typographic_quotes(""), "")
        self.assertEqual(DEMUX.typographic_quotes("bez cudzysłowów"), "bez cudzysłowów")

    def test_peer_send_publishes_writers_quotes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            DEMUX.write_channel_binding(root, "1", "vagabond", "claude-code", "sender-session",
                                        str(root / "bus-1.jsonl"))
            DEMUX.write_channel_binding(root, "2", "lena", "codex", "lena-session",
                                        str(root / "bus-2.jsonl"))
            lease_id = DEMUX.lease_identifier("claude-code", "sender-session")
            DEMUX.atomic_json(root / "leases" / f"{lease_id}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "provider": "claude-code",
                "provider_session_id": "sender-session", "lease_id": lease_id,
                "name": "vagabond", "bus": str(root / "bus-1.jsonl")})
            args = argparse.Namespace(bridge_home=root, provider="claude-code",
                                      session="sender-session",
                                      send='Zobacz "to", proszę, i `cs-bus --send "x"`', to="lena")
            with patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                 patch.object(DEMUX, "live_follower_pid", return_value=321), \
                 patch.object(DEMUX, "emit"):
                self.assertEqual(DEMUX.send_peer_command(args), 0)
            self.assertEqual(publish.call_args.args[1]["text"],
                             'Zobacz „to”, proszę, i `cs-bus --send "x"`')

    def test_say_publishes_writers_quotes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            args = argparse.Namespace(
                bridge_home=root, provider="claude-code", session="say-session", name="klaudiusz",
                say='Ścieżka "główna" to `~/.codescribe/"x"`', voice=None, speed=None,
                tts_vendor=None, reply_to=None)
            with patch.object(DEMUX, "reply_delivery_envelope", return_value=(root / "bus.jsonl", None)), \
                 patch.object(DEMUX, "voice_profile", return_value={"voice": "leo", "speed": 1.0}), \
                 patch.object(DEMUX, "publish_reply_event", return_value=RECEIPT) as publish, \
                 patch.object(DEMUX, "atomic_json"), patch.object(DEMUX, "emit"), \
                 patch.object(DEMUX, "speak_published_reply", return_value=0):
                self.assertEqual(DEMUX.say_reply(args), 0)
            self.assertEqual(publish.call_args.args[1]["text"],
                             'Ścieżka „główna” to `~/.codescribe/"x"`')


if __name__ == "__main__":
    unittest.main()
