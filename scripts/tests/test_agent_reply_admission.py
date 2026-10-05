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
    def test_damaged_bundle_never_discovers_an_unverified_publisher(self):
        for damage in ('missing-publisher', 'missing-manifest', 'wrong-schema', 'missing-entry', 'dangling-publisher'):
            with self.subTest(damage=damage), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                runtime = root / 'runtime'
                (runtime / 'bin').mkdir(parents=True)
                publisher = runtime / 'bin/codescribe'
                manifest = runtime / 'manifest.json'
                publisher.write_bytes(b'publisher')
                publisher.chmod(0o700)
                manifest.write_text(json.dumps({'schema': 'codescribe.agent-bridge.bundle.v1', 'files': []}))
                if damage == 'missing-publisher':
                    publisher.unlink()
                elif damage == 'missing-manifest':
                    manifest.unlink()
                elif damage == 'wrong-schema':
                    manifest.write_text(json.dumps({'schema': 'foreign', 'files': []}))
                elif damage == 'dangling-publisher':
                    publisher.unlink()
                    publisher.symlink_to(runtime / 'absent')
                with patch('shutil.which', return_value='/unverified/codescribe') as discovery:
                    with self.assertRaises(ValueError):
                        DEMUX.reply_publisher_command(root)
                    discovery.assert_not_called()

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
        envelope = {**owner, "kind": "seal", "delivery_id": identity, "source_event_id": "physical-1",
                    "utterance_id": "physical-1", "session_id": "capture-a",
                    "capture_epoch": 1, "document_index": 0, "audience": "lena",
                    "recipients": [owner], "text": "Pytanie."}
        path = self.home / "leases" / f"{lease_id}.json"
        path.parent.mkdir()
        state = {"schema": DEMUX.LEASE_SCHEMA, **owner, "cursor": 0, "pending": [envelope]}
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
        with self.assertRaises(RuntimeError):
            self.say(lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("speaker died")))
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
            with self.assertRaises(OSError):
                DEMUX.say_reply(self.args)
            speaker.assert_not_called()

    def test_equal_text_replies_keep_distinct_occurrence_ids(self):
        for _ in range(5):
            self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
        replies = [row for row in self.rows() if row.get("kind") == "agent_reply"]
        self.assertEqual(len(replies), 5)
        self.assertEqual(len({row["reply_id"] for row in replies}), 5)
        self.assertEqual({row["text"] for row in replies}, {self.args.say})

    def test_stop_controls_only_the_running_owned_ticket_and_retains_text(self):
        def speaker(*_args, **kwargs):
            control = kwargs["control"]
            self.args.stop_reply = control.reply["reply_id"]
            self.args.playback_ticket = control.ticket
            self.assertEqual(DEMUX.stop_reply_command(self.args), 0)
            self.assertTrue(control.stopped())
            return False, "stopped", "stopped"

        self.assertEqual(self.say(speaker), 5)
        rows = self.rows()
        self.assertEqual(rows[0]["text"], self.args.say)
        self.assertEqual(rows[-1]["state"], "stopped")
        self.assertEqual(rows[0]["reply_id"], rows[-1]["reply_id"])

    def test_foreign_session_cannot_stop_the_running_reply(self):
        def speaker(*_args, **kwargs):
            control = kwargs["control"]
            foreign = argparse.Namespace(**vars(self.args))
            foreign.session = "other-session"
            foreign.stop_reply = control.reply["reply_id"]
            foreign.playback_ticket = control.ticket
            with self.assertRaises(ValueError):
                DEMUX.stop_reply_command(foreign)
            self.assertFalse(control.stopped())
            return True, None, None

        self.assertEqual(self.say(speaker), 0)
        self.assertEqual(self.rows()[-1]["state"], "spoken")

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


    def test_renamed_reply_defaults_to_frozen_recipient_voice_profile(self):
        identity, _, _ = self.admitted_delivery()
        self.args.reply_to, self.args.name = identity, "renamed"
        self.args.voice = self.args.speed = self.args.tts_vendor = None
        with patch.object(DEMUX, "voice_profile", return_value={
                "voice": "eve", "speed": 1.2, "provider": "xai"}) as profile:
            self.assertEqual(self.say(lambda *_args, **_kwargs: (True, None, None)), 0)
            profile.assert_called_once_with(self.home, "lena")
        reply = next(row for row in self.rows() if row["kind"] == "agent_reply")
        self.assertEqual((reply["name"], reply["voice"], reply["speed"]), ("lena", "eve", 1.2))

    def test_enriched_ack_rejects_foreign_owner_envelope_and_frozen_recipient(self):
        identity, _, state = self.admitted_delivery()
        self.args.ack = [identity]
        with contextlib.redirect_stdout(io.StringIO()):
            DEMUX.acknowledge_delivery(self.args)
        lease = state["lease_id"]
        marker = self.home / "acknowledgments" / lease / f"{identity}.json"
        valid = json.loads(marker.read_text())
        self.assertTrue(DEMUX.delivery_acknowledged(self.home, lease, identity))
        for target, key, value in [("marker", "provider", "claude"),
                ("marker", "provider_session_id", "other"), ("marker", "bus", "/other"),
                ("envelope", "lease_id", "wrong"), ("envelope", "delivery_id", "b" * 24),
                ("envelope", "bus", "/other"), ("recipient", "provider_session_id", "other")]:
            with self.subTest(target=target, key=key):
                bad = json.loads(json.dumps(valid))
                subject = bad if target == "marker" else bad["envelope"]
                if target == "recipient":
                    subject = subject["recipients"][0]
                subject[key] = value
                marker.write_text(json.dumps(bad))
                self.assertFalse(DEMUX.delivery_acknowledged(self.home, lease, identity))
        marker.write_text(json.dumps({"lease_id": lease, "delivery_id": identity, "provider": "codex"}))
        self.assertFalse(DEMUX.delivery_acknowledged(self.home, lease, identity))
        self.args.lease_ttl = 30
        with contextlib.redirect_stdout(io.StringIO()) as output:
            DEMUX.status_command(self.args)
        self.assertEqual(json.loads(output.getvalue())["backlog"], 1)
        marker.write_text(json.dumps({"lease_id": lease, "delivery_id": identity}))
        self.assertTrue(DEMUX.delivery_acknowledged(self.home, lease, identity))


    def test_five_equal_text_occurrences_remain_five_refused_documents(self):
        normalizer = DEMUX.EvidenceNormalizer()
        for occurrence in range(5):
            normalizer.normalize({"schema": DEMUX.EVIDENCE_SCHEMA,
                "session_id": "agent-channel-2-five", "audience": "lena",
                "reducer_action": "apply_ledger_decision", "reducer_revision": occurrence,
                "document_index": occurrence, "capture_epoch": 1,
                "sample_start": occurrence * 16000, "sample_end": (occurrence + 1) * 16000,
                "rendered_text": "Iwo"})
        normalizer.normalize({"schema": DEMUX.CLEAN_SCHEMA,
            "session_id": "agent-channel-2-five", "status": DEMUX.SESSION_ENDED})
        rows = normalizer.pop_flushes()
        self.assertEqual(len(rows), 5)
        self.assertEqual(len({row["source_event_id"] for row in rows}), 5)
        self.assertEqual({row["text"] for row in rows}, {"Iwo"})
        self.assertTrue(all(row["coverage"] == "refused" for row in rows))


    def test_foreign_phase_ack_cannot_discard_a_newly_queued_owned_delivery(self):
        identity, path, state = self.admitted_delivery()
        self.args.ack = [identity]
        with contextlib.redirect_stdout(io.StringIO()):
            DEMUX.acknowledge_delivery(self.args)
        payload = state["pending"][0]
        state["pending"] = []
        path.write_text(json.dumps(state))
        marker = self.home / "acknowledgments" / state["lease_id"] / f"{identity}.json"
        receipt = json.loads(marker.read_text())
        receipt["envelope"]["kind"] = "draft"
        marker.write_text(json.dumps(receipt))
        lease = DEMUX.SessionLease(root=self.home, provider="codex",
            provider_session_id=self.args.session, name="lena", bus=self.bus,
            requested_id=None, ttl_seconds=30, follow_from_end=False)
        self.addCleanup(lease.close)
        self.assertTrue(lease.queue_delivery(payload), "a mismatched phase receipt discarded a seal")
        lease.collect_acknowledgments()
        self.assertIn(identity, lease.pending)


if __name__ == "__main__":
    unittest.main()
