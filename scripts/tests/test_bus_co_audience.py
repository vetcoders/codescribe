"""Co-audience survives delivery projections without becoming routing authority."""
import argparse
import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

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

    def close(self, normalizer):
        normalizer.normalize({"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "channel": "3",
            "session_id": self.evidence()["session_id"], "state": "closed",
            "emitted_at": "2026-10-25T04:00:10Z"})
        rows = normalizer.pop_flushes()
        self.assertEqual(len(rows), 1)
        self.assertNotIn("_co_audience_observation", rows[0])
        return rows[0]

    def test_same_phase_close_observation_reaches_final_delivery(self):
        normalizer = DEMUX.EvidenceNormalizer()
        first = self.evidence()
        normalizer.normalize(first)
        latest = self.evidence(sequence=2, co_audience=[first["co_audience"][1]],
            co_audience_basis="capture_close")
        self.assertIsNone(normalizer.normalize(latest))
        final = self.close(normalizer)
        self.assertEqual(final["co_audience"], latest["co_audience"])
        self.assertEqual(final["co_audience_basis"], "capture_close")
        self.assertEqual(final["text"], first["rendered_text"])
        self.assertEqual(final["recipients"], first["recipients"])

    def test_session_ended_retains_its_frozen_close_membership(self):
        normalizer = DEMUX.EvidenceNormalizer()
        first = self.evidence()
        normalizer.normalize(first)
        ending = self.evidence(sequence=3, reducer_action=DEMUX.SESSION_ENDED,
            co_audience=[first["co_audience"][1]], co_audience_basis="capture_close")
        normalizer.normalize(ending)
        final = normalizer.pop_flushes()[0]
        self.assertEqual(final["co_audience"], ending["co_audience"])
        self.assertEqual(final["co_audience_basis"], "capture_close")
        self.assertNotIn("_co_audience_observation", final)

    def test_complete_same_phase_empty_snapshot_clears_previous_membership(self):
        normalizer = DEMUX.EvidenceNormalizer()
        normalizer.normalize(self.evidence())
        empty = self.evidence(sequence=2)
        empty.pop("co_audience")
        empty.pop("co_audience_basis")
        normalizer.normalize(empty)
        final = self.close(normalizer)
        self.assertNotIn("co_audience", final)
        self.assertNotIn("co_audience_basis", final)

    def test_new_epoch_does_not_inherit_previous_observation_watermark(self):
        normalizer = DEMUX.EvidenceNormalizer()
        first = self.evidence(sequence=100)
        normalizer.normalize(first)
        next_epoch = self.evidence(capture_epoch=8, sequence=3, reducer_revision=2,
            co_audience=[first["co_audience"][1]])
        normalizer.normalize(next_epoch)
        closing = self.evidence(capture_epoch=8, sequence=4, reducer_revision=2,
            co_audience=[first["co_audience"][0]], co_audience_basis="capture_close")
        normalizer.normalize(closing)
        final = self.close(normalizer)
        self.assertEqual(final["capture_epoch"], 8)
        self.assertEqual(final["co_audience"], closing["co_audience"])
        self.assertEqual(final["co_audience_basis"], "capture_close")

    def test_delayed_emission_cannot_overwrite_later_close_observation(self):
        normalizer = DEMUX.EvidenceNormalizer()
        first = self.evidence()
        normalizer.normalize(first)
        closed = self.evidence(sequence=3, co_audience=[first["co_audience"][1]],
            co_audience_basis="capture_close")
        normalizer.normalize(closed)
        normalizer.normalize(self.evidence(sequence=2))
        self.assertEqual(self.close(normalizer)["co_audience"], closed["co_audience"])

    def test_queue_context_is_bounded_sanitized_and_not_a_recipient_list(self):
        row = self.evidence(co_audience=[{"channel": "9", "audience": "Olga\nspoof"}] * 12,
                            co_audience_basis="capture_close")
        before = copy.deepcopy(row)
        label = DEMUX.co_audience_queue_label(row)
        self.assertTrue(label.startswith("OTHER LIVE CHANNELS AT CAPTURE CLOSE (context only):"))
        self.assertNotIn("\n", label)
        self.assertEqual(label.count("9 ("), 9)
        self.assertEqual(row, before)

    def test_dual_channel_primary_and_projection_seats_wake_and_queue_only_once_while_preserving_history(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "bridge"
            bus = (Path(tmp) / "transcript-events.jsonl").resolve(strict=False)
            provider = "codex"
            session = "roman-thread"
            lease_id = DEMUX.lease_identifier(provider, session)

            primary_session = "agent-channel-3-dual-seat-primary"
            projection_session = "agent-channel-7-dual-seat-projection"
            capture_session_id = "capture-shared-epoch-42"

            recipients_both = [
                {
                    "channel": "3",
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "bus": str(bus),
                    "delivery_seat": "primary",
                },
                {
                    "channel": "7",
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "bus": str(bus),
                    "delivery_seat": "projection",
                },
            ]

            bus_rows = [
                {
                    "schema": DEMUX.CHANNEL_SESSION_SCHEMA,
                    "channel": "3",
                    "session_id": primary_session,
                    "capture_session_id": capture_session_id,
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "state": "open",
                    "publication": "controller",
                    "opened_at": "2026-10-25T04:00:00Z",
                    "emitted_at": "2026-10-25T04:00:00Z",
                },
                {
                    "schema": DEMUX.CHANNEL_RECIPIENTS_SCHEMA,
                    "channel": "3",
                    "session_id": primary_session,
                    "capture_session_id": capture_session_id,
                    "delivery_seat": "primary",
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "recipients": recipients_both,
                    "emitted_at": "2026-10-25T04:00:00Z",
                },
                {
                    "schema": DEMUX.CHANNEL_SESSION_SCHEMA,
                    "channel": "7",
                    "session_id": projection_session,
                    "capture_session_id": capture_session_id,
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "state": "open",
                    "publication": "controller",
                    "opened_at": "2026-10-25T04:00:00Z",
                    "emitted_at": "2026-10-25T04:00:00Z",
                },
                {
                    "schema": DEMUX.CHANNEL_RECIPIENTS_SCHEMA,
                    "channel": "7",
                    "session_id": projection_session,
                    "capture_session_id": capture_session_id,
                    "delivery_seat": "projection",
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "recipients": recipients_both,
                    "emitted_at": "2026-10-25T04:00:00Z",
                },
                self.evidence(
                    session_id=primary_session,
                    channel="3",
                    sequence=1,
                    rendered_text="Tylko jedno wybudzenie dla Romana.",
                    status=DEMUX.SEALED,
                    closing_decision=DEMUX.CLOSING_PUBLISH,
                    recipients=recipients_both,
                ),
                self.evidence(
                    session_id=projection_session,
                    channel="7",
                    sequence=2,
                    rendered_text="Tylko jedno wybudzenie dla Romana.",
                    status=DEMUX.SEALED,
                    closing_decision=DEMUX.CLOSING_PUBLISH,
                    recipients=recipients_both,
                ),
                {
                    "schema": DEMUX.CHANNEL_SESSION_SCHEMA,
                    "channel": "3",
                    "session_id": primary_session,
                    "capture_session_id": capture_session_id,
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "state": "closed",
                    "reason": "manual",
                    "publication": "controller",
                    "participation": {
                        "capture_session_id": capture_session_id,
                        "segment_session_id": primary_session,
                        "channel": "3",
                        "delivery_seat": "primary",
                    },
                    "opened_at": "2026-10-25T04:00:00Z",
                    "closed_at": "2026-10-25T04:00:05Z",
                    "emitted_at": "2026-10-25T04:00:05Z",
                },
                {
                    "schema": DEMUX.CHANNEL_SESSION_SCHEMA,
                    "channel": "7",
                    "session_id": projection_session,
                    "capture_session_id": capture_session_id,
                    "audience": "roman",
                    "provider": provider,
                    "provider_session_id": session,
                    "lease_id": lease_id,
                    "state": "closed",
                    "reason": "manual",
                    "publication": "controller",
                    "participation": {
                        "capture_session_id": capture_session_id,
                        "segment_session_id": projection_session,
                        "channel": "7",
                        "delivery_seat": "projection",
                    },
                    "opened_at": "2026-10-25T04:00:00Z",
                    "closed_at": "2026-10-25T04:00:05Z",
                    "emitted_at": "2026-10-25T04:00:05Z",
                },
            ]
            bus.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in bus_rows), encoding="utf-8")

            normalizer = DEMUX.EvidenceNormalizer()
            flushed = []
            for raw in bus.read_text(encoding="utf-8").splitlines():
                parsed = DEMUX.parse_line(raw)
                self.assertIsNotNone(parsed)
                normalizer.normalize(parsed)
                flushed.extend(normalizer.pop_flushes())

            self.assertEqual(len(flushed), 2)
            by_session = {row["session_id"]: row for row in flushed}
            self.assertEqual(by_session[primary_session]["delivery_seat"], "primary")
            self.assertEqual(by_session[projection_session]["delivery_seat"], "projection")

            lease = DEMUX.SessionLease(
                root=root,
                provider=provider,
                provider_session_id=session,
                name="roman",
                bus=bus,
                requested_id=None,
                ttl_seconds=90.0,
                follow_from_end=False,
                coalesce=True,
            )
            try:
                woken = []
                for event in flushed:
                    self.assertEqual(
                        lease.admits_channel_event(event),
                        event["session_id"] == primary_session,
                    )
                    considered = DEMUX.consider(
                        event,
                        name="roman",
                        hear_all=False,
                        drafts=False,
                        debug=False,
                    )
                    if event["session_id"] == projection_session:
                        self.assertIsNone(considered)
                        projection_payload = DEMUX.slim(event, "roman")
                        projection_payload.update(
                            status=DEMUX.SEALED,
                            delivery_id="c" * 24,
                            lease_id=lease_id,
                            provider=provider,
                            provider_session_id=session,
                            bus=str(bus),
                            delivery_owner=lease.owner_Metadata() if hasattr(lease, "owner_Metadata") else {
                                "lease_id": lease_id,
                                "provider": provider,
                                "provider_session_id": session,
                                "name": "roman",
                                "channel": "7",
                                "pid": 1234,
                            },
                        )
                        self.assertFalse(lease.queue_delivery(projection_payload))
                        self.assertIsNone(DEMUX.watch_line(projection_payload, lease_id))
                    else:
                        self.assertIsNotNone(considered)
                        considered["delivery_id"] = DEMUX.delivery_identity(lease_id, considered)
                        considered["lease_id"] = lease_id
                        considered["provider"] = provider
                        considered["provider_session_id"] = session
                        considered["bus"] = str(bus)
                        considered["delivery_owner"] = {
                            "lease_id": lease_id,
                            "provider": provider,
                            "provider_session_id": session,
                            "name": "roman",
                            "channel": "3",
                            "pid": 1234,
                        }
                        self.assertTrue(lease.queue_delivery(considered))
                        watch = DEMUX.watch_line(considered, lease_id)
                        self.assertIsNotNone(watch)
                        woken.append(watch)

                self.assertEqual(len(lease.pending), 1)
                self.assertEqual(len(woken), 1)
                only_pending = next(iter(lease.pending.values()))
                self.assertEqual(only_pending["session_id"], primary_session)
                self.assertEqual(only_pending["delivery_seat"], "primary")

                read_args = argparse.Namespace(
                    bridge_home=root,
                    bus=bus,
                    bus_overridden=True,
                    provider=provider,
                    session=session,
                )
                _, pending_rows = DEMUX.unread_pending(read_args, include_quiet=True)
                self.assertEqual(len(pending_rows), 1)
                self.assertEqual(pending_rows[0]["session_id"], primary_session)

                (root / "buses").mkdir(mode=0o700, parents=True, exist_ok=True)
                (root / "buses" / "channel-3.jsonl").write_text(
                    "".join(
                        json.dumps(row, ensure_ascii=False) + "\n"
                        for row in bus_rows
                        if row.get("channel") == "3"
                    ),
                    encoding="utf-8",
                )
                (root / "buses" / "channel-7.jsonl").write_text(
                    "".join(
                        json.dumps(row, ensure_ascii=False) + "\n"
                        for row in bus_rows
                        if row.get("channel") == "7"
                    ),
                    encoding="utf-8",
                )
                messages, _, skipped = DEMUX.history_messages(root)
                self.assertEqual(skipped, [])
                voice_rows = [row for row in messages if row.get("lane") == "voice"]
                self.assertEqual(len(voice_rows), 2)
                self.assertEqual({next(iter(row["channels"])) for row in voice_rows}, {"3", "7"})
            finally:
                lease._release_lock()


if __name__ == "__main__":
    unittest.main()
