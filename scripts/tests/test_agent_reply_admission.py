"""Integrator acceptance: reply durability is independent of speech.

All storage is temporary. Speakers are substituted; no audio or network is used.
"""

import argparse
import contextlib
import importlib.util
import io
import json
import os
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
        self.root = Path(temporary.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.home = self.root / "agent-bridge"
        self.home.mkdir()
        self.args = argparse.Namespace(
            bridge_home=self.home, bus=self.bus, name="lena", provider="codex",
            session="session-a", say="Odpowiedź przed syntezą.", voice="eve",
            speed=1.0, tts_vendor="xai", reply_to=None, bus_overridden=True,
        )
        scoped = patch.object(DEMUX, "bus_path", return_value=self.bus)
        scoped.start()
        self.addCleanup(scoped.stop)
        publisher = patch.object(DEMUX, "publish_reply_event", side_effect=self.publish,
                                 create=True)
        publisher.start()
        self.addCleanup(publisher.stop)

    def publish(self, bus, event, *, bridge_root=None):
        # A synchronous durable storage boundary. Rust CLI acceptance separately
        # exercises the real generation/chunk publisher; this fixture isolates
        # the helper's ordering, ownership and speech behavior.
        self.assertEqual(bridge_root, self.home)
        raw = (json.dumps(event, ensure_ascii=False) + "\n").encode()
        with bus.open("ab") as handle:
            offset = handle.tell()
            handle.write(raw)
            handle.flush()
            os.fsync(handle.fileno())
            stat = os.fstat(handle.fileno())
        return {"stream_id": "fixture-stream", "stream_dev": stat.st_dev, "stream_inode": stat.st_ino,
                "offset": offset, "length": len(raw)}

    def admitted_delivery(self):
        identity = "a" * 24
        lease_id = DEMUX.lease_identifier(self.args.provider, self.args.session)
        owner = {"provider": self.args.provider, "provider_session_id": self.args.session,
                 "lease_id": lease_id, "bus": str(self.bus), "channel": 2,
                 "name": "lena"}
        envelope = {**owner, "delivery_id": identity, "source_event_id": "physical-1",
                    "utterance_id": "physical-1", "session_id": "capture-a",
                    "capture_epoch": 1, "document_index": 0, "audience": "lena",
                    "recipients": [owner], "text": "Pytanie."}
        path = self.home / "leases" / f"{lease_id}.json"
        path.parent.mkdir()
        state = {"schema": DEMUX.LEASE_SCHEMA, **owner, "pending": [envelope]}
        path.write_text(json.dumps(state))
        return identity, path, state

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
        outcomes = [row for row in self.rows() if row.get("kind") == "agent_reply_playback"]
        self.assertTrue(outcomes)
        self.assertEqual(outcomes[-1]["state"], "failed", "a dead speaker must not leave a playing ticket")
        self.assertEqual(outcomes[-1]["reply_id"], replies[0]["reply_id"])

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

    def test_no_reply_target_never_guesses_the_pending_question(self):
        self.admitted_delivery()
        self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        reply = next(row for row in self.rows() if row["kind"] == "agent_reply")
        self.assertEqual(reply["association"], "unsolicited")
        self.assertIsNone(reply["delivery_id"])

    def test_reply_uses_the_admitted_owner_name_after_rename(self):
        identity, _, _ = self.admitted_delivery()
        self.args.reply_to = identity
        self.args.name = "new-name"
        self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        reply = next(row for row in self.rows() if row["kind"] == "agent_reply")
        self.assertEqual((reply["association"], reply["delivery_id"], reply["name"]),
                         ("addressed", identity, "lena"))
        self.assertEqual(reply["source_event_id"], "physical-1")

    def test_unknown_delivery_is_refused_before_publication_and_speech(self):
        self.admitted_delivery()
        self.args.reply_to = "b" * 24
        with patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as speaker:
            with self.assertRaises(ValueError):
                DEMUX.say_reply(self.args)
            speaker.assert_not_called()
        self.assertEqual(self.rows(), [])

    def test_other_session_cannot_answer_the_owned_delivery(self):
        identity, path, state = self.admitted_delivery()
        state["pending"][0]["provider_session_id"] = "session-b"
        path.write_text(json.dumps(state))
        self.args.reply_to = identity
        with patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as speaker:
            with self.assertRaises(ValueError):
                DEMUX.say_reply(self.args)
            speaker.assert_not_called()
        self.assertEqual(self.rows(), [])

    def test_late_broadcast_recipient_cannot_answer_an_earlier_utterance(self):
        identity, path, state = self.admitted_delivery()
        state["pending"][0]["recipients"] = [
            {"provider": "codex", "provider_session_id": "other-session",
             "lease_id": DEMUX.lease_identifier("codex", "other-session"),
             "bus": str(self.bus), "channel": 2, "name": "lena"}]
        path.write_text(json.dumps(state))
        self.args.reply_to = identity
        with patch.object(DEMUX, "_speak_xai", return_value=(True, None, None)) as speaker:
            with self.assertRaises(ValueError):
                DEMUX.say_reply(self.args)
            speaker.assert_not_called()
        self.assertEqual(self.rows(), [])

    def test_acknowledgment_pruning_keeps_causal_reply_ownership(self):
        identity, path, state = self.admitted_delivery()
        self.args.ack = [identity]
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args), 0)
        state["pending"] = []
        path.write_text(json.dumps(state))
        self.args.reply_to = identity
        self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        reply = next(row for row in self.rows() if row["kind"] == "agent_reply")
        self.assertEqual((reply["delivery_id"], reply["source_event_id"]),
                         (identity, "physical-1"))


if __name__ == "__main__":
    unittest.main()
