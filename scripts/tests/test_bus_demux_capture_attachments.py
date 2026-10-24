"""Voice attachment admission/projection; local files only, no app or network."""
import copy
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "capture_attachment_demux_test", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class CaptureAttachmentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.bus = Path(self.temp.name) / "primary.jsonl"
        self.bus.touch()
        self.session = "agent-channel-0-fixture"
        self.recipients = [{"provider": "codex", "provider_session_id": "one", "lease_id": "a" * 32,
                            "channel": "1", "audience": "one", "bus": str(self.bus)}]
        self.pointer = {"path": str(Path(self.temp.name) / "image.png"), "name": "image.png",
                        "media_type": "image/png", "bytes": 13, "sha256": "a" * 64}
        self.anchored_pointer = {**self.pointer, "attached_at_sample": 10, "attached_at_seconds": 0.0}
        self.stage = {"schema": DEMUX.CAPTURE_ATTACHMENT_SCHEMA, "kind": "capture_attachment",
                      "stage_id": "1" * 24, "session_id": self.session, "capture_epoch": 7,
                      "source_sequence": 0, "commit_bus": str(self.bus), "audience": "all",
                      "reducer_revision": 1, "occurrence_session_id": self.session,
                      "sample_start": 10, "sample_end": 100, "document_index": 0,
                      "recipients": self.recipients, "attachments": [self.pointer]}

    def evidence(self, **changes):
        row = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": self.session, "sequence": 0,
               "capture_epoch": 7, "reducer_revision": 1, "reducer_action": "record_ledger_projection",
               "occurrence_session_id": self.session, "sample_start": 10, "sample_end": 100,
               "document_index": 0, "rendered_text": "Voice words", "channel": "0",
               "audience": "all", "recipients": self.recipients, "emitted_at": "2026-10-20T22:00:00Z"}
        row.update(changes)
        return row

    def commit(self, **changes):
        row = {key: self.stage[key] for key in
               ("stage_id", "session_id", "capture_epoch", "source_sequence", "commit_bus")}
        row.update(schema=DEMUX.CAPTURE_ATTACHMENT_COMMIT_SCHEMA, kind="capture_attachment_commit")
        row.update(changes)
        self.bus.write_text(json.dumps(row) + "\n")
        return row

    def seal(self, normalizer):
        normalizer.normalize(self.evidence(sequence=1, reducer_revision=2,
                                          reducer_action=DEMUX.TERMINAL_SEAL))
        normalizer.normalize({"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "channel": "0",
                              "session_id": self.session, "state": "closed"})
        return normalizer.pop_flushes()

    def test_committed_stage_is_part_of_one_sealed_voice_message_after_revision(self):
        normalizer = DEMUX.EvidenceNormalizer()
        normalizer.normalize(self.evidence())
        normalizer.normalize(self.stage)
        self.commit()
        sealed = self.seal(normalizer)
        self.assertEqual(len(sealed), 1)
        self.assertEqual(sealed[0]["attachments"], [self.anchored_pointer])
        self.assertEqual(sealed[0]["recipients"], self.recipients)
        self.assertEqual(sealed[0]["session_id"], self.session)
        self.assertIn("Voice words\n[attachment @ 0:00] image.png", sealed[0]["text"])
        self.assertEqual(DEMUX.slim(sealed[0], "all")["attachments"], [self.anchored_pointer])

    def test_partial_fanout_or_foreign_commit_does_not_attach(self):
        for changes in (None, {"capture_epoch": 8}, {"source_sequence": 1}, {"stage_id": "2" * 24}):
            with self.subTest(changes=changes):
                self.bus.write_text("")
                if changes is not None:
                    self.commit(**changes)
                normalizer = DEMUX.EvidenceNormalizer()
                normalizer.normalize(self.evidence())
                normalizer.normalize(self.stage)
                sealed = self.seal(normalizer)
                self.assertEqual(len(sealed), 1)
                self.assertNotIn("attachments", sealed[0])
                self.assertEqual(sealed[0]["text"], "Voice words")

    def test_paste_keeps_its_source_after_continued_speech_revises_the_same_range(self):
        normalizer = DEMUX.EvidenceNormalizer()
        normalizer.normalize(self.evidence())
        normalizer.normalize(self.evidence(sequence=1, reducer_revision=2,
                                          rendered_text="More voice words"))
        normalizer.normalize(self.stage)
        self.commit()
        sealed = self.seal(normalizer)
        self.assertEqual(sealed[0]["attachments"], [self.anchored_pointer])
        self.assertNotIn("capture_attachment_sources", sealed[0])

    def test_pasted_source_survives_restart_after_newer_projection(self):
        original = DEMUX.EvidenceNormalizer()
        original.normalize(self.evidence())
        original.normalize(self.evidence(sequence=1, reducer_revision=2,
                                         sample_start=10, sample_end=100))
        restored = DEMUX.EvidenceNormalizer()
        restored.restore_channel_documents(json.loads(json.dumps(list(original.channel_documents().values()))))
        restored.normalize(self.stage)
        self.commit()
        self.assertEqual(self.seal(restored)[0]["attachments"], [self.anchored_pointer])

    def test_stale_target_or_recipient_change_cannot_retarget_file(self):
        self.commit()
        for key, wrong in (("source_sequence", 9), ("capture_epoch", 8),
                           ("audience", "foreign"), ("recipients", [])):
            with self.subTest(key=key):
                normalizer = DEMUX.EvidenceNormalizer()
                normalizer.normalize(self.evidence())
                stage = copy.deepcopy(self.stage)
                stage[key] = wrong
                normalizer.normalize(stage)
                self.assertNotIn("attachments", self.seal(normalizer)[0])

    def test_pending_stage_survives_lease_restart_and_deduplicates(self):
        original = DEMUX.EvidenceNormalizer()
        original.normalize(self.evidence())
        original.normalize(self.stage)
        original.normalize(self.stage)
        restored = DEMUX.EvidenceNormalizer()
        restored.restore_channel_documents(list(original.channel_documents().values()))
        self.commit()
        self.assertEqual(self.seal(restored)[0]["attachments"], [self.anchored_pointer])

    def test_two_stages_use_the_capture_rate_and_take_origin_for_different_anchors(self):
        normalizer = DEMUX.EvidenceNormalizer()
        rate = 48_000
        first_sample = 12_000
        normalizer.normalize(self.evidence(sample_start=first_sample, sample_end=24_000))
        normalizer.normalize(self.stage)
        normalizer.normalize(self.evidence(sequence=1, reducer_revision=2,
                                          sample_start=first_sample + rate * 73,
                                          sample_end=first_sample + rate * 74,
                                          seal_coverage={"sample_rate_hz": rate}))
        second = {**self.stage, "stage_id": "2" * 24, "source_sequence": 1,
                  "attachments": [{**self.pointer, "name": "image2.png"}]}
        normalizer.normalize(second)
        first_commit = self.commit()
        second_commit = {**first_commit, "stage_id": second["stage_id"], "source_sequence": 1}
        self.bus.write_text(json.dumps(first_commit) + "\n" + json.dumps(second_commit) + "\n")
        sealed = self.seal(normalizer)[0]
        self.assertEqual([item["attached_at_sample"] for item in sealed["attachments"]],
                         [first_sample, first_sample + rate * 73])
        self.assertEqual([item["attached_at_seconds"] for item in sealed["attachments"]], [0.0, 73.0])
        self.assertIn("[attachment @ 0:00] image.png", sealed["text"])
        self.assertIn("[attachment @ 1:13] image2.png", sealed["text"])

    def test_missing_source_keeps_committed_pointer_at_zero(self):
        normalizer = DEMUX.EvidenceNormalizer()
        normalizer.normalize(self.evidence())
        stage = {**self.stage, "source_sequence": 99}
        normalizer.normalize(stage)
        self.commit(source_sequence=99)
        sealed = self.seal(normalizer)[0]
        self.assertEqual(sealed["attachments"],
                         [{**self.pointer, "attached_at_sample": 0, "attached_at_seconds": 0.0}])
        self.assertIn("[attachment @ 0:00] image.png", sealed["text"])

    def test_unavailable_clock_never_assumes_the_vad_sample_rate(self):
        normalizer = DEMUX.EvidenceNormalizer()
        normalizer.normalize(self.evidence())
        normalizer.normalize(self.evidence(sequence=2, reducer_revision=2,
                                          sample_start=960_010, sample_end=1_008_010))
        stage = {**self.stage, "source_sequence": 2}
        normalizer.normalize(stage)
        self.commit(source_sequence=2)
        pointer = self.seal(normalizer)[0]["attachments"][0]
        self.assertEqual(pointer["attached_at_sample"], 960_010)
        self.assertEqual(pointer["attached_at_seconds"], 0.0)

    def test_exact_commit_is_read_through_storage_chunk_decoder(self):
        commit = self.commit()
        encoded = json.dumps(commit).encode()
        chunk = {"schema": "codescribe.bus-chunk.v1", "part": 0, "parts": 1,
                 "length": len(encoded), "id": hashlib.sha256(encoded).hexdigest(),
                 "event": {"schema": DEMUX.CAPTURE_ATTACHMENT_COMMIT_SCHEMA},
                 "payload": base64.b64encode(encoded).decode()}
        self.bus.write_text(json.dumps(chunk) + "\n")
        self.assertTrue(DEMUX.capture_attachment_is_committed(self.stage))
        chunk["id"] = "0" * 64
        self.bus.write_text(json.dumps(chunk) + "\n")
        self.assertFalse(DEMUX.capture_attachment_is_committed(self.stage))

    def test_reply_reference_validation_and_queue_projection_preserve_parent(self):
        parent = {"message_id": "reply:fixture", "author": "Astra", "text": "Quoted context"}
        self.assertEqual(DEMUX.validate_user_reply_reference(parent), parent)
        row = {"schema": "codescribe.agent-user-message.v1", "kind": "agent_user_message",
               "text": "Answer", "user_reply_to": parent, "message_id": "a" * 24}
        self.assertEqual(DEMUX.EvidenceNormalizer().normalize(row)["user_reply_to"], parent)
        self.assertEqual(DEMUX.slim(row, "astra")["user_reply_to"], parent)
        with self.assertRaises(ValueError):
            DEMUX.validate_user_reply_reference({**parent, "message_id": "bad\nidentity"})


if __name__ == "__main__":
    unittest.main()
