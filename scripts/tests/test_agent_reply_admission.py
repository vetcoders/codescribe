"""Integrator acceptance: reply durability is independent of speech.

All storage is temporary. Speakers are substituted; no audio or network is used.
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
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "agent_reply_admission", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class AgentReplyAdmissionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bus = self.root / "bus.jsonl"
        self.home = self.root / "agent-bridge"
        self.home.mkdir()
        self.args = argparse.Namespace(
            bridge_home=self.home, bus=self.bus, name="lena", provider="codex",
            session="session-a", say="Odpowiedź przed syntezą.", voice="eve",
            speed=1.0, tts_vendor="xai", reply_to=None, unsolicited=True,
        )
        scoped = patch.object(DEMUX, "bus_path", return_value=self.bus)
        scoped.start()
        self.addCleanup(scoped.stop)

    def rows(self):
        if not self.bus.exists():
            return []
        return [json.loads(line) for line in self.bus.read_text().splitlines()]

    def say(self, speaker):
        with patch.object(DEMUX, "_speak_xai", side_effect=speaker), \
                contextlib.redirect_stdout(io.StringIO()):
            return DEMUX.say_reply(self.args)

    def test_reply_is_durable_before_speaker_is_entered(self):
        def speaker(*_args, **_kwargs):
            replies = [row for row in self.rows() if row.get("kind") == "agent_reply"]
            self.assertEqual(len(replies), 1, "TTS started before text publication")
            self.assertEqual(replies[0]["text"], self.args.say)
            self.assertFalse(replies[0]["spoken"])
            return True, None, None

        self.assertEqual(self.say(speaker), 0)

    def test_unexpected_speaker_failure_keeps_the_admitted_text(self):
        try:
            self.say(lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("speaker died")))
        except RuntimeError:
            pass
        replies = [row for row in self.rows() if row.get("kind") == "agent_reply"]
        self.assertEqual(len(replies), 1)
        self.assertEqual(replies[0]["text"], self.args.say)
        self.assertFalse(replies[0]["spoken"])

    def test_publication_failure_never_calls_the_speaker(self):
        self.bus.mkdir()
        with patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as speaker, \
                contextlib.redirect_stdout(io.StringIO()):
            try:
                DEMUX.say_reply(self.args)
            except OSError:
                pass
            speaker.assert_not_called()

    def test_equal_text_replies_keep_distinct_occurrence_ids(self):
        for _ in range(5):
            self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        replies = [row for row in self.rows() if row.get("kind") == "agent_reply"]
        self.assertEqual(len(replies), 5)
        self.assertEqual(len({row["reply_id"] for row in replies}), 5)
        self.assertEqual({row["text"] for row in replies}, {self.args.say})


if __name__ == "__main__":
    unittest.main()
