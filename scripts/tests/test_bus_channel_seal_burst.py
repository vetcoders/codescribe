"""One spoken channel take reaches the attached conversation exactly once.

Regression for the seal storm seen live on 2026-10-05 (x2, x3, x11 in one
session): the producer persisted each reducer publication as one
``shared-revision.v1`` row whose ``occurrence_rows`` named every PCM document
of the take, and the follower flushed one coverage-refused seal per document
entry, each with the full take text and its own delivery id. These tests run
the real follower over that row shape and count what the conversation sees.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("bus_channel_seal_burst", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

PROVIDER, CONVERSATION, NAME, CHANNEL = "claude-code", "conversation-a", "jozek", "1"
WINDOW = 32768


class ChannelSealBurstTests(unittest.TestCase):
    def setUp(self):
        self.reset()

    def reset(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name)
        self.root = self.home / "agent-bridge"
        self.bus = self.home / "channel-1.jsonl"
        self.bus.touch()
        self.lease_id = DEMUX.lease_identifier(PROVIDER, CONVERSATION)
        self.recipients = [{"audience": NAME, "bus": str(self.bus.resolve()), "channel": CHANNEL,
                            "lease_id": self.lease_id, "name": NAME, "provider": PROVIDER,
                            "provider_session_id": CONVERSATION}]
        self.sequence = 0

    def coordinates(self, session, index):
        self.sequence += 1
        start = 4096 + index * WINDOW
        return {"sequence": self.sequence, "capture_epoch": 1, "sample_start": start,
                "sample_end": start + WINDOW - 2048, "document_index": index,
                "emitted_at": f"2026-10-05T12:57:{self.sequence % 60:02d}.000000Z",
                "occurrence_session_id": session, "label": "Iwo",
                "acoustic_receipts": [{"receipt_id": f"pcm-{session}-{index}"}]}

    def publication(self, session, revision, action, text, primary, carried):
        row = {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session, "audience": NAME,
               "channel": CHANNEL, "mode": "agent", "recipients": self.recipients,
               "reducer_action": action, "reducer_revision": revision, "rendered_text": text,
               "persistence_encoding": "shared-revision.v1", **self.coordinates(session, primary)}
        row["occurrence_rows"] = [self.coordinates(session, index) for index in carried]
        return row

    def channel_row(self, session, state, opened_at):
        return {"schema": DEMUX.CHANNEL_SESSION_SCHEMA, "kind": "channel_session", "channel": CHANNEL,
                "session_id": session, "state": state, "reason": "opened" if state == "open" else "hangup",
                "opened_at": opened_at, "emitted_at": opened_at, "agent": NAME, "provider": PROVIDER,
                "provider_session_id": CONVERSATION, "recipients": self.recipients}

    def take(self, session, documents, text, opened_at="2026-10-05T12:55:25.436000Z"):
        """Rows of one take: a publication per admitted document, then a seal
        phase whose every publication re-projects all document entries."""
        rows = [self.channel_row(session, "open", opened_at)]
        revision = 0
        for index in range(documents):
            revision += 1
            rows.append(self.publication(session, revision, "apply_ledger_decision",
                                         " ".join(["Iwo"] * (index + 1)), index, []))
        for action in ("seal_coverage", "seal_coverage", "seal_coverage", "apply_manual_edit"):
            revision += 1
            rows.append(self.publication(session, revision, action, text, 0, range(1, documents)))
        return rows, self.channel_row(session, "sealed", opened_at)

    def append(self, *rows):
        with self.bus.open("a", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row, ensure_ascii=False) + "\n")

    def run_demux(self, *extra):
        return subprocess.run([sys.executable, str(SPEC.origin), "--bus", str(self.bus), "--bridge-home",
                               str(self.root), "--provider", PROVIDER, "--session", CONVERSATION, *extra],
                              capture_output=True, text=True, timeout=30, check=True,
                              env={**os.environ, "CODESCRIBE_DATA_DIR": str(self.home)})

    def follow(self):
        """One follower pass over the unread bus, as the attached --coalesce rail runs it."""
        result = self.run_demux("--name", NAME, "--from-start", "--drafts", "--coalesce")
        return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]

    def surfaced(self, envelopes):
        """Envelopes the conversation is woken for; previews stay in the mailbox."""
        return [payload for payload in envelopes if DEMUX.watch_line(payload, self.lease_id) is not None]

    def pending(self):
        return DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")["pending"]

    @staticmethod
    def ranges(payload):
        return {(item["occurrence_session_id"], item["capture_epoch"], item["sample_start"], item["sample_end"])
                for item in payload["occurrences"]}

    def test_direct_terminal_closes_once_without_acoustic_refusal(self):
        session = "agent-channel-1-direct-translation"
        row = self.publication(session, 2, "apply_direct_transcript_terminal",
                               "Iwo Iwo Iwo Iwo Iwo", 0, range(1, 5))
        row.update(status=DEMUX.SEALED, terminal=True, coverage="direct",
                   hold_until="2000-01-01T00:00:00.000000Z")
        row["acoustic_receipts"] = []
        for occurrence in row["occurrence_rows"]:
            occurrence["acoustic_receipts"] = []
        hangup = self.channel_row(session, "closed", "2026-10-17T10:00:00Z")
        self.append(row, hangup)
        surfaced = self.surfaced(self.follow())
        self.assertEqual(len(surfaced), 1)
        self.assertEqual(surfaced[0]["text"], "Iwo Iwo Iwo Iwo Iwo")
        self.assertEqual(surfaced[0].get("coverage"), "direct")
        self.assertEqual(surfaced[0].get("hold_until"), row["hold_until"])
        self.assertTrue(all(not item.get("acoustic_receipts") for item in surfaced[0]["occurrences"]))
        replay = self.surfaced(self.follow())
        self.assertEqual([item["delivery_id"] for item in replay], [surfaced[0]["delivery_id"]],
                         "an unacknowledged replay retains the same delivery identity")
        self.assertEqual(sum(item.get("status") == DEMUX.SEALED for item in self.pending()), 1)

    def test_successor_open_waits_for_predecessor_decoder_tail_and_named_close(self):
        old, new = "agent-channel-1-draining", "agent-channel-1-successor"
        old_open = self.channel_row(old, "open", "2026-10-17T12:40:34Z")
        new_open = self.channel_row(new, "open", "2026-10-17T12:40:50Z")
        first = self.publication(old, 1, "apply_direct_transcript", "Iwo Iwo", 0, [])
        first["acoustic_receipts"] = []
        self.append(old_open, first, new_open)
        self.assertEqual(self.surfaced(self.follow()), [],
                         "a successor capture is not authority to deliver its predecessor")
        complete = self.publication(old, 3, "apply_direct_transcript_terminal",
                                    "Iwo Iwo Iwo Iwo Iwo", 0, range(1, 5))
        complete["acoustic_receipts"] = []
        for entry in complete["occurrence_rows"]:
            entry["acoustic_receipts"] = []
        self.append(complete)
        self.assertEqual(self.surfaced(self.follow()), [],
                         "retained decoder tail still waits for its named close")
        close = self.channel_row(old, "sealed", "2026-10-17T12:40:51Z")
        close["reason"] = "silence"
        self.append(close)
        (sealed,) = self.surfaced(self.follow())
        self.assertEqual(sealed["session_id"], old)
        self.assertEqual(sealed["text"], "Iwo Iwo Iwo Iwo Iwo")
        self.assertEqual(sealed["coverage"], "direct")
        self.assertEqual(len(self.ranges(sealed)), 5)
        self.assertEqual(sum(row.get("kind") == "seal" for row in self.pending()), 1)

    def test_successor_open_keeps_full_tail_inside_the_original_send_hold(self):
        old, new = "agent-channel-1-held-tail", "agent-channel-1-next-held"
        partial = self.publication(old, 1, "apply_direct_transcript", "first words", 0, [])
        complete = self.publication(old, 3, "apply_direct_transcript_terminal", "first words and real tail", 0, [1])
        complete.update(status=DEMUX.SEALED, terminal=True, coverage="direct",
                        hold_until="2099-10-17T12:41:03.000000Z")
        self.append(self.channel_row(old, "open", "2026-10-17T12:40:00Z"), partial,
                    self.channel_row(new, "open", "2026-10-17T12:40:50Z"), complete,
                    self.channel_row(old, "sealed", "2026-10-17T12:40:51Z"))
        self.assertEqual(self.surfaced(self.follow()), [])
        lease = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        self.assertEqual(len(lease["held"]), 1)
        (held,) = lease["held"].values()
        self.assertEqual(held["text"], "first words and real tail")
        self.assertEqual(held["hold_until"], complete["hold_until"])
        self.assertFalse(any(row.get("kind") == "seal" for row in self.pending()))

    def test_explicit_orphan_close_recovers_only_its_named_capture(self):
        session = "agent-channel-1-orphan-drain"
        row = self.publication(session, 1, "apply_direct_transcript", "retained words", 0, [])
        self.append(self.channel_row(session, "open", "2026-10-17T12:40:00Z"), row)
        self.assertEqual(self.surfaced(self.follow()), [])
        close = self.channel_row(session, "sealed", "2026-10-17T12:45:00Z")
        close["reason"] = "orphan"
        self.append(close)
        (sealed,) = self.surfaced(self.follow())
        self.assertEqual(sealed["text"], "retained words")
        self.assertEqual(sealed["coverage"], DEMUX.COVERAGE_REFUSED)

    def test_direct_nonchannel_terminal_is_a_single_final_phase(self):
        normalizer = DEMUX.EvidenceNormalizer()
        row = self.publication("dictation-direct", 7, "apply_direct_transcript_terminal",
                               "English survives.", 0, [])
        row.pop("audience")
        row["acoustic_receipts"] = []
        clean = normalizer.normalize(row)
        self.assertEqual(clean["status"], DEMUX.SEALED)
        self.assertEqual(clean.get("coverage"), "direct")
        self.assertIsNone(normalizer.normalize(row))

    def test_storm_of_n_documents_is_one_delivery_with_every_pcm_entry(self):
        for documents in (2, 3, 11):
            with self.subTest(documents=documents):
                self.reset()
                session = f"agent-channel-1-take-{documents}"
                text = " ".join(["Iwo"] * (documents + 3))
                rows, hangup = self.take(session, documents, text)
                self.append(*rows, hangup)
                surfaced = self.surfaced(self.follow())
                self.assertEqual(len(surfaced), 1, [item.get("delivery_id") for item in surfaced])
                seal = surfaced[0]
                self.assertEqual((seal["kind"], seal["status"]), ("seal", DEMUX.SEALED))
                self.assertEqual(seal["text"], text)
                self.assertEqual(seal["coverage"], DEMUX.COVERAGE_REFUSED)
                self.assertFalse(seal["state_change_allowed"])
                self.assertEqual(len(self.ranges(seal)), documents, "every PCM document stays evidence")
                kinds = sorted(item["kind"] for item in self.pending())
                self.assertEqual(kinds, ["revised", "seal"], "the storm folds into one preview and one seal")

    def test_unacknowledged_take_replays_under_one_identity_until_ack(self):
        rows, hangup = self.take("agent-channel-1-ack", 11, "Iwo Iwo Iwo")
        self.append(*rows, hangup)
        first = self.surfaced(self.follow())
        self.assertEqual(len(first), 1)
        replay = self.surfaced(self.follow())
        self.assertEqual([item["delivery_id"] for item in replay], [first[0]["delivery_id"]])
        self.run_demux("--ack", first[0]["delivery_id"])
        self.assertEqual(self.surfaced(self.follow()), [])
        self.assertFalse(any(item["kind"] == "seal" for item in self.pending()))

    def test_restart_inside_the_storm_still_delivers_once(self):
        rows, hangup = self.take("agent-channel-1-restart", 11, "Iwo Iwo")
        self.append(*rows[:8])
        self.assertEqual(self.surfaced(self.follow()), [])
        self.append(*rows[8:])
        self.assertEqual(self.surfaced(self.follow()), [])
        self.append(hangup)
        final = self.surfaced(self.follow())
        self.assertEqual(len(final), 1)
        self.assertEqual(len(self.ranges(final[0])), 11)
        self.assertEqual([item["delivery_id"] for item in self.surfaced(self.follow())], [final[0]["delivery_id"]])

    def test_equal_text_in_separate_takes_is_never_folded(self):
        first_rows, first_close = self.take("agent-channel-1-first", 3, "Iwo Iwo Iwo")
        second_rows, second_close = self.take("agent-channel-1-second", 3, "Iwo Iwo Iwo",
                                              opened_at="2026-10-05T12:58:25.436000Z")
        self.append(*first_rows, first_close, *second_rows, second_close)
        surfaced = self.surfaced(self.follow())
        self.assertEqual([item["session_id"] for item in surfaced], ["agent-channel-1-first", "agent-channel-1-second"])
        self.assertEqual(len({item["delivery_id"] for item in surfaced}), 2)
        self.run_demux("--ack", surfaced[0]["delivery_id"])
        replay = self.surfaced(self.follow())
        self.assertEqual([item["delivery_id"] for item in replay], [surfaced[1]["delivery_id"]],
                         "acknowledging one take never settles another with the same words")


if __name__ == "__main__":
    unittest.main()
