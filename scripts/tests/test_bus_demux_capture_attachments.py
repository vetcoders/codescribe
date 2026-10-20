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
        self.stage = {"schema": DEMUX.CAPTURE_ATTACHMENT_SCHEMA, "kind": "capture_attachment",
                      "stage_id": "1" * 24, "session_id": self.session, "capture_epoch": 7,
                      "source_sequence": 0, "commit_bus": str(self.bus), "audience": "all",
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
        self.assertEqual(sealed[0]["attachments"], [self.pointer])
        self.assertEqual(sealed[0]["recipients"], self.recipients)
        self.assertEqual(sealed[0]["session_id"], self.session)
        self.assertIn("Voice words\n[attachment] image.png", sealed[0]["text"])
        self.assertEqual(DEMUX.slim(sealed[0], "all")["attachments"], [self.pointer])

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
        self.assertEqual(self.seal(restored)[0]["attachments"], [self.pointer])

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
