"""A finished message leaves exactly one envelope in the follower mailbox.

Seen live on 2026-10-06: nothing acknowledges a draft, so the last preview of
every message stayed in the lease for the life of the session. Leases grew to
hundreds of kilobytes (the app's roster then read their followers as absent),
`pending` marched toward the 256-envelope cap, and every resume re-published
previews of messages that had long been sealed and acknowledged.

The rule: acknowledging a message's terminal envelope settles its drafts.
Until then every envelope, drafts included, stays replayable and can be
acknowledged on its own, which is what a `--drafts` pipe reader relies on.

The first class drives the real lease mailbox with envelopes shaped like the
follower's own; the second runs the real follower over channel takes.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("bus_draft_retirement", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

PROVIDER, CONVERSATION, NAME, CHANNEL = "claude-code", "conversation-a", "jozek", "1"
TAKE = "agent-channel-1-take"
OTHER_TAKE = "agent-channel-1-other"
NAMED = "named-session"


class MailboxTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name) / "agent-bridge"
        self.bus = Path(temp.name) / "bus.jsonl"
        self.bus.touch()
        self.minted = 0
        self.lease = self.open_lease()

    def open_lease(self, coalesce=False):
        lease = DEMUX.SessionLease(root=self.root, provider=PROVIDER, provider_session_id=CONVERSATION,
                                   name=NAME, bus=self.bus, requested_id=None, ttl_seconds=30,
                                   follow_from_end=False, coalesce=coalesce)
        self.addCleanup(lease._release_lock)
        return lease

    def reopen(self, coalesce=False):
        self.lease._release_lock()
        self.lease = self.open_lease(coalesce)

    def envelope(self, kind, session, *, message_id=None, document_index=None, text="Iwo"):
        self.minted += 1
        payload = {"kind": kind, "session_id": session, "audience": NAME, "text": text,
                   "delivery_id": f"{self.minted:024x}", "lease_id": self.lease.lease_id,
                   "provider": PROVIDER, "provider_session_id": CONVERSATION, "bus": self.lease.bus}
        if message_id is not None:
            payload["message_id"] = message_id
        if document_index is not None:
            payload["document_index"] = document_index
        return payload

    def queue(self, *args, **kwargs):
        payload = self.envelope(*args, **kwargs)
        self.assertTrue(self.lease.queue_delivery(payload))
        return payload

    def kinds(self):
        return [item["kind"] for item in self.lease.pending.values()]

    def stored(self):
        return DEMUX.read_json(self.lease.path)["pending"]

    def acknowledge(self, payload, **overrides):
        marker = {"lease_id": self.lease.lease_id, "delivery_id": payload["delivery_id"],
                  "provider": PROVIDER, "provider_session_id": CONVERSATION, "bus": self.lease.bus,
                  "envelope": DEMUX.receipt_envelope(payload, self.lease.bus)}
        marker.update(overrides)
        DEMUX.atomic_json(self.root / "acknowledgments" / self.lease.lease_id / f"{payload['delivery_id']}.json", marker)

    def leave_behind(self, *payloads):
        """Write the mailbox an earlier helper left on disk, then resume it."""
        self.lease.pending = {payload["delivery_id"]: payload for payload in payloads}
        self.lease.persist(active=False)
        self.reopen()
        self.assertEqual([item["delivery_id"] for item in self.lease.pending.values()],
                         [payload["delivery_id"] for payload in payloads])

    def settle(self, payload):
        self.acknowledge(payload)
        self.lease.collect_acknowledgments()

    def test_an_unacknowledged_message_keeps_every_envelope_in_order(self):
        queued = [self.queue("draft", NAMED, document_index=1), self.queue("revised", NAMED, document_index=1),
                  self.queue("seal", NAMED, document_index=1)]
        self.lease.collect_acknowledgments()
        self.assertEqual(list(self.lease.pending), [item["delivery_id"] for item in queued],
                         "a pipe reader replays and acknowledges drafts too; queueing the seal drops nothing")
        self.assertEqual([item["delivery_id"] for item in self.stored()], [item["delivery_id"] for item in queued])

    def test_acknowledging_the_seal_settles_every_draft_of_its_message(self):
        self.queue("draft", NAMED, document_index=1)
        self.queue("revised", NAMED, document_index=1)
        self.settle(self.queue("seal", NAMED, document_index=1))
        self.assertEqual(self.lease.pending, {})
        self.assertEqual(self.stored(), [], "the drafts leave in the same persisted step as their seal")

    def test_acknowledging_the_seal_settles_the_coalesced_preview(self):
        self.reopen(coalesce=True)
        self.queue("draft", NAMED, document_index=1)
        newest = self.queue("revised", NAMED, document_index=1)
        seal = self.queue("seal", NAMED, document_index=1)
        self.assertEqual(list(self.lease.pending), [newest["delivery_id"], seal["delivery_id"]])
        self.settle(seal)
        self.assertEqual(self.lease.pending, {})

    def test_a_channel_take_is_keyed_by_its_message_identity(self):
        message = DEMUX.channel_message_identity(TAKE)
        for index in range(3):
            self.queue("revised", TAKE, message_id=message, document_index=index)
        refused = self.envelope("seal", TAKE, message_id=message)
        refused.update(coverage=DEMUX.COVERAGE_REFUSED, state_change_allowed=False)
        self.assertTrue(self.lease.queue_delivery(refused))
        self.settle(refused)
        self.assertEqual(self.lease.pending, {},
                         "one take is one message however many PCM documents it holds, "
                         "and a coverage-refused seal still closes it")

    def test_another_document_of_the_same_session_keeps_its_draft(self):
        kept = self.queue("revised", NAMED, document_index=1)
        self.queue("revised", NAMED, document_index=2)
        self.settle(self.queue("seal", NAMED, document_index=2))
        self.assertEqual(list(self.lease.pending), [kept["delivery_id"]])

    def test_another_session_keeps_its_draft(self):
        kept = self.queue("revised", OTHER_TAKE, message_id=DEMUX.channel_message_identity(OTHER_TAKE))
        self.queue("revised", TAKE, message_id=DEMUX.channel_message_identity(TAKE))
        self.settle(self.queue("seal", TAKE, message_id=DEMUX.channel_message_identity(TAKE)))
        self.assertEqual(list(self.lease.pending), [kept["delivery_id"]])

    def test_an_acknowledged_typed_message_settles_no_draft(self):
        draft = self.queue("revised", NAMED, document_index=1)
        self.settle(self.queue("message", NAMED, message_id=f"{0xabc:024x}"))
        self.assertEqual(list(self.lease.pending), [draft["delivery_id"]])

    def test_an_acknowledged_draft_settles_only_itself(self):
        draft = self.queue("draft", NAMED, document_index=1)
        revised = self.queue("revised", NAMED, document_index=1)
        seal = self.queue("seal", NAMED, document_index=1)
        self.settle(draft)
        self.assertEqual(list(self.lease.pending), [revised["delivery_id"], seal["delivery_id"]])

    def test_a_codex_seal_settles_its_drafts_only_once_its_native_queue_entry_is_gone(self):
        self.lease._release_lock()
        self.lease = DEMUX.SessionLease(root=self.root, provider="codex", provider_session_id=CONVERSATION,
                                        name=NAME, bus=self.bus, requested_id=None, ttl_seconds=30,
                                        follow_from_end=False)
        self.addCleanup(self.lease._release_lock)
        draft = self.envelope("revised", NAMED, document_index=1)
        seal = self.envelope("seal", NAMED, document_index=1)
        for payload in (draft, seal):
            payload["provider"] = "codex"
            self.assertTrue(self.lease.queue_delivery(payload))
        receipt = self.root / "wakeups" / self.lease.lease_id / f"{seal['delivery_id']}.json"
        DEMUX.atomic_json(receipt, {"queue_disposition": "pending"})
        marker = {"lease_id": self.lease.lease_id, "delivery_id": seal["delivery_id"], "provider": "codex",
                  "provider_session_id": CONVERSATION, "bus": self.lease.bus,
                  "envelope": DEMUX.receipt_envelope(seal, self.lease.bus)}
        DEMUX.atomic_json(self.root / "acknowledgments" / self.lease.lease_id / f"{seal['delivery_id']}.json", marker)
        self.lease.collect_acknowledgments()
        self.assertEqual(list(self.lease.pending), [draft["delivery_id"], seal["delivery_id"]])
        DEMUX.atomic_json(receipt, {"queue_disposition": "removed"})
        self.lease.collect_acknowledgments()
        self.assertEqual(self.lease.pending, {})

    def test_a_full_mailbox_refuses_the_seal_and_keeps_every_draft(self):
        for index in range(256):
            self.queue("revised", NAMED, document_index=index)
        before_memory = list(self.lease.pending)
        before_disk = [item["delivery_id"] for item in self.stored()]
        with self.assertRaises(BufferError):
            self.lease.queue_delivery(self.envelope("seal", NAMED, document_index=0))
        self.assertEqual(list(self.lease.pending), before_memory)
        self.assertEqual([item["delivery_id"] for item in self.stored()], before_disk)

    def test_resume_drops_a_draft_whose_seal_was_acknowledged_earlier(self):
        message = DEMUX.channel_message_identity(TAKE)
        orphan = self.envelope("revised", TAKE, message_id=message)
        seal = self.envelope("seal", TAKE, message_id=message)
        self.leave_behind(orphan)
        self.acknowledge(seal)
        self.lease.prune_settled_drafts()
        self.assertEqual(self.lease.pending, {})
        self.assertEqual(self.stored(), [])

    def test_resume_keeps_a_draft_without_proof_that_its_message_closed(self):
        message = DEMUX.channel_message_identity(TAKE)
        other = DEMUX.channel_message_identity(OTHER_TAKE)
        cases = {
            "no marker store at all": lambda: None,
            "the marker is for a draft": lambda: self.acknowledge(self.envelope("revised", TAKE, message_id=message)),
            "the marker closes another message": lambda: self.acknowledge(
                self.envelope("seal", OTHER_TAKE, message_id=other)),
            "the marker belongs to another conversation": lambda: self.acknowledge(
                self.envelope("seal", TAKE, message_id=message), provider_session_id="conversation-b"),
        }
        for label, arrange in cases.items():
            with self.subTest(label):
                orphan = self.envelope("revised", TAKE, message_id=message)
                self.leave_behind(orphan)
                arrange()
                self.lease.prune_settled_drafts()
                self.assertEqual(list(self.lease.pending), [orphan["delivery_id"]])

    def test_resume_keeps_an_unacknowledged_seal_and_what_an_earlier_helper_left_beside_it(self):
        message = DEMUX.channel_message_identity(TAKE)
        draft = self.envelope("revised", TAKE, message_id=message)
        seal = self.envelope("seal", TAKE, message_id=message)
        self.leave_behind(draft, seal)
        self.lease.prune_settled_drafts()
        self.assertEqual(list(self.lease.pending), [draft["delivery_id"], seal["delivery_id"]],
                         "only an acknowledgment proves a message settled")

    def test_one_key_for_previews_and_their_terminal_envelope(self):
        message = DEMUX.channel_message_identity(TAKE)
        self.assertEqual(DEMUX.draft_key({"session_id": TAKE, "message_id": message, "document_index": 4}),
                         DEMUX.draft_key({"session_id": TAKE, "message_id": message}))
        self.assertNotEqual(DEMUX.draft_key({"session_id": NAMED, "document_index": 1}),
                            DEMUX.draft_key({"session_id": NAMED, "document_index": 2}))
        self.assertEqual(DEMUX.draft_key({"session_id": NAMED, "document_index": 0}), (NAMED, 0))


class FollowerTests(unittest.TestCase):
    WINDOW = 32768

    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.home = Path(temp.name)
        self.root = self.home / "agent-bridge"
        self.bus = self.home / "channel-1.jsonl"
        self.bus.touch()
        self.lease_id = DEMUX.lease_identifier(PROVIDER, CONVERSATION)
        self.lease_path = self.root / "leases" / f"{self.lease_id}.json"
        self.recipients = [{"audience": NAME, "bus": str(self.bus.resolve()), "channel": CHANNEL,
                            "lease_id": self.lease_id, "name": NAME, "provider": PROVIDER,
                            "provider_session_id": CONVERSATION}]
        self.sequence = 0

    def coordinates(self, session, index):
        self.sequence += 1
        start = 4096 + index * self.WINDOW
        return {"sequence": self.sequence, "capture_epoch": 1, "sample_start": start,
                "sample_end": start + self.WINDOW - 2048, "document_index": index,
                "emitted_at": f"2026-10-06T10:00:{self.sequence % 60:02d}.000000Z",
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

    def speak(self, session, opened_at):
        """One three-document channel take, from open to hangup."""
        rows = [self.channel_row(session, "open", opened_at)]
        for index in range(3):
            rows.append(self.publication(session, index + 1, "apply_ledger_decision",
                                         " ".join(["Iwo"] * (index + 1)), index, []))
        rows.append(self.publication(session, 4, "seal_coverage", "Iwo Iwo Iwo", 0, range(1, 3)))
        rows.append(self.channel_row(session, "sealed", opened_at))
        with self.bus.open("a", encoding="utf-8") as stream:
            for row in rows:
                stream.write(json.dumps(row, ensure_ascii=False) + "\n")

    def run_demux(self, *extra):
        return subprocess.run([sys.executable, str(SPEC.origin), "--bus", str(self.bus), "--bridge-home",
                               str(self.root), "--provider", PROVIDER, "--session", CONVERSATION, *extra],
                              capture_output=True, text=True, timeout=30, check=True,
                              env={**os.environ, "CODESCRIBE_DATA_DIR": str(self.home)})

    def follow(self):
        result = self.run_demux("--name", NAME, "--from-start", "--drafts", "--coalesce")
        return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]

    def state(self):
        return DEMUX.read_json(self.lease_path)

    def test_a_long_session_leaves_only_unacknowledged_seals_in_the_mailbox(self):
        acknowledged = []
        for number in range(6):
            self.speak(f"agent-channel-1-take-{number}", f"2026-10-06T10:{number:02d}:00.000000Z")
            seals = [item for item in self.follow() if item.get("kind") == "seal"
                     and item["delivery_id"] not in acknowledged]
            self.assertEqual(len(seals), 1)
            self.assertEqual(sorted(item["kind"] for item in self.state()["pending"]), ["revised", "seal"],
                             "only the newest take waits; acknowledged takes took their previews with them")
            self.run_demux("--ack", seals[0]["delivery_id"])
            acknowledged.append(seals[0]["delivery_id"])
        self.follow()
        state = self.state()
        self.assertEqual(state["pending"], [])
        self.assertLess(self.lease_path.stat().st_size, 16 * 1024)

    def test_resume_neither_replays_nor_restores_a_preview_of_a_settled_take(self):
        session = "agent-channel-1-settled"
        self.speak(session, "2026-10-06T10:00:00.000000Z")
        first = self.follow()
        preview = [item for item in first if item.get("kind") in DEMUX.DRAFT_KINDS][-1]
        seal = next(item for item in first if item.get("kind") == "seal")
        self.run_demux("--ack", seal["delivery_id"])
        self.follow()
        state = self.state()
        self.assertEqual(state["pending"], [])
        # The mailbox an earlier helper left behind: the acknowledged seal is
        # gone, its last preview is still there.
        state["pending"] = [preview]
        state["active"] = False
        DEMUX.atomic_json(self.lease_path, state)
        replayed = self.follow()
        self.assertNotIn(preview["delivery_id"], [item.get("delivery_id") for item in replayed])
        state = self.state()
        self.assertEqual(state["pending"], [])
        self.assertNotIn(session, state["unclosed_channel_messages"],
                         "a settled take is not resurrected as an unclosed document")


if __name__ == "__main__":
    unittest.main()
