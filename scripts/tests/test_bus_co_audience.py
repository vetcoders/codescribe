"""Co-audience survives delivery projections without becoming routing authority."""
import copy
import importlib.util
from pathlib import Path
import sys
import unittest

SPEC = importlib.util.spec_from_file_location(
    "co_audience_demux_test", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class CoAudienceTests(unittest.TestCase):
    def evidence(self, **changes):
        row = {
            "schema": DEMUX.EVIDENCE_SCHEMA,
            "session_id": "agent-channel-3-co-audience-fixture",
            "sequence": 1, "capture_epoch": 7, "reducer_revision": 1,
            "reducer_action": "record_ledger_projection",
            "occurrence_session_id": "occurrence-fixture",
            "sample_start": 10, "sample_end": 100, "document_index": 0,
            "rendered_text": "Dokładnie te same słowa.", "channel": "3",
            "audience": "roman", "emitted_at": "2026-10-25T04:00:00Z",
            "recipients": [{"channel": "3", "audience": "roman", "provider": "codex",
                            "provider_session_id": "roman-thread", "lease_id": "a" * 32}],
            "co_audience": [{"channel": "1", "audience": "klaudiusz",
                             "session_id": "capture-klaudiusz", "provider": "claude-code",
                             "provider_session_id": "klaudiusz-thread"},
                            {"channel": "9", "audience": "olga-div0",
                             "session_id": "capture-olga", "provider": "codex",
                             "provider_session_id": "olga-thread"}],
            "co_audience_basis": "emission",
        }
        row.update(changes)
        return row

    def test_full_evidence_to_owned_read_and_watch_preserves_both_fields(self):
        for basis in ("emission", "capture_close"):
            with self.subTest(basis=basis):
                row = self.evidence(co_audience_basis=basis)
                clean = DEMUX.EvidenceNormalizer().normalize(row)
                self.assertIsNotNone(clean)
                delivery = DEMUX.slim(clean, "roman")
                delivery.update(status=DEMUX.SEALED, delivery_id="b" * 24, lease_id="a" * 32,
                                provider="codex", provider_session_id="roman-thread")
                # Every public envelope stage must preserve the producer's meaning.
                projections = [clean, delivery, DEMUX.conversation_envelope(delivery),
                               DEMUX.watch_line(delivery, "a" * 32)]
                for projected in projections:
                    self.assertIsNotNone(projected)
                    self.assertEqual(projected["co_audience"], row["co_audience"])
                    self.assertEqual(projected["co_audience_basis"], basis)
                    self.assertEqual(projected["channel"], "3")
                    self.assertEqual(projected["text"], row["rendered_text"])
                for projected in projections[:3]:
                    self.assertEqual(projected["recipients"], row["recipients"])
                self.assertEqual(projections[2]["delivery_id"], "b" * 24)
                self.assertEqual(projections[3]["lease_id"], "a" * 32)

    def test_later_revision_uses_its_membership_not_the_first_rows(self):
        normalizer = DEMUX.EvidenceNormalizer()
        first = self.evidence()
        self.assertEqual(normalizer.normalize(first)["co_audience"], first["co_audience"])
        later = self.evidence(sequence=2, reducer_revision=2,
                              co_audience=[first["co_audience"][1]],
                              co_audience_basis="capture_close")
        revised = normalizer.normalize(later)
        self.assertEqual(revised["co_audience"], later["co_audience"])
        self.assertEqual(revised["co_audience_basis"], "capture_close")

    def test_membership_does_not_change_source_identity_text_or_authority(self):
        plain = self.evidence()
        plain.pop("co_audience")
        plain.pop("co_audience_basis")
        annotated = self.evidence()
        self.assertEqual(DEMUX.source_event_identity(plain),
                         DEMUX.source_event_identity(annotated))
        for status in (DEMUX.SEALED, "transcript_revised"):
            base = DEMUX.slim({**plain, "status": status, "text": plain["rendered_text"]}, "roman")
            tagged = DEMUX.slim({**annotated, "status": status,
                                  "text": annotated["rendered_text"]}, "roman")
            for key in ("source_event_id", "text", "state_change_allowed", "recipients", "channel"):
                self.assertEqual(base[key], tagged[key])

    def test_co_audience_cannot_address_a_delivery_to_a_peer(self):
        clean = DEMUX.EvidenceNormalizer().normalize(self.evidence())
        clean.update(status=DEMUX.SEALED, text="Dokładnie te same słowa.")
        before = copy.deepcopy(clean)
        self.assertIsNone(DEMUX.consider(clean, name="klaudiusz", hear_all=False,
                                        drafts=False, debug=False, recipients={"roman", "klaudiusz"}))
        own = DEMUX.consider(clean, name="roman", hear_all=False,
                             drafts=False, debug=False, recipients={"roman", "klaudiusz"})
        self.assertIsNotNone(own)
        self.assertEqual(own["audience"], "roman")
        self.assertEqual(own["recipients"], clean["recipients"])
        self.assertEqual(clean, before)

    def test_plain_single_recipient_keeps_optional_metadata_absent(self):
        row = self.evidence()
        row.pop("co_audience")
        row.pop("co_audience_basis")
        clean = DEMUX.EvidenceNormalizer().normalize(row)
        delivery = DEMUX.slim(clean, "roman")
        delivery.update(status=DEMUX.SEALED, delivery_id="b" * 24, lease_id="a" * 32)
        for projected in (clean, delivery, DEMUX.conversation_envelope(delivery),
                          DEMUX.watch_line(delivery, "a" * 32)):
            self.assertNotIn("co_audience", projected)
            self.assertNotIn("co_audience_basis", projected)


if __name__ == "__main__":
    unittest.main()
