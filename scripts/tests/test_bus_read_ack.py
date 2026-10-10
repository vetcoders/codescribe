"""A bell is not a read: drain complete owned envelopes, then acknowledge."""
import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_read_ack", SOURCE)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class ReadAckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.session = "11111111-2222-4333-8444-555555555555"
        self.lease = DEMUX.lease_identifier("codex", self.session)
        self.ids = [f"{i:024x}" for i in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        self.save()

    def envelope(self, identity):
        owner = dict(lease_id=self.lease, provider="codex", provider_session_id=self.session)
        return dict(owner, delivery_owner=owner, delivery_id=identity,
                    kind="message", source="typed", text="Iwo", bus=str(self.bus),
                    emitted_at="2026-10-07T13:37:33Z", message_id=identity)

    def save(self):
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease, "provider": "codex",
            "provider_session_id": self.session, "bus": str(self.bus),
            "pending": self.pending, "cursor": 42,
        })

    def args(self, **extra):
        return argparse.Namespace(bridge_home=self.root, provider="codex", session=self.session,
                                  bus=self.bus, bus_overridden=True, read_limit=8,
                                  read_bytes=65536, **extra)

    def read(self, **limits):
        args = self.args()
        for key, value in limits.items():
            setattr(args, key, value)
        with patch.object(DEMUX, "emit") as emit:
            self.assertEqual(DEMUX.read_pending_command(args), 0)
        return emit.call_args.args[0]

    def ack(self, identities):
        with patch.object(DEMUX, "delete_native_queue_submission", return_value=True), \
             patch.object(DEMUX, "emit"):
            self.assertEqual(DEMUX.acknowledge_delivery(self.args(ack=identities)), 0)

    def bounded_watch(self, *extra):
        return subprocess.run(
            [sys.executable, str(SOURCE), "--watch", "--until-event",
             "--provider", "codex", "--session", self.session,
             "--bridge-home", str(self.root), "--max-wait", "0.05", *extra],
            capture_output=True, text=True, timeout=5)

    def test_bounded_watch_wakes_for_existing_mailbox_without_reading_or_acknowledging(self):
        before = (self.root / "leases" / f"{self.lease}.json").read_bytes()
        result = self.bounded_watch()
        self.assertEqual(result.returncode, 0, result.stderr)
        notice = json.loads(result.stdout)
        self.assertEqual(notice["kind"], "mailbox_ready")
        self.assertEqual(notice["pending_count"], 5)
        self.assertNotIn("read_delivery_ids", notice)
        self.assertNotIn("Iwo", result.stdout)
        self.assertEqual((self.root / "leases" / f"{self.lease}.json").read_bytes(), before)
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_bounded_watch_ignores_acked_messages_and_drafts(self):
        self.ack(self.ids)
        self.pending.append(dict(self.envelope("e" * 24), kind="revised"))
        self.save()
        result = self.bounded_watch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["kind"], "watch_timeout")

    def test_bounded_watch_wakes_on_arrival_during_wait(self):
        import threading
        self.pending = []
        self.save()
        def arrive():
            self.pending = [self.envelope(self.ids[0])]
            self.save()
        timer = threading.Timer(0.15, arrive)
        timer.start()
        try:
            result = self.bounded_watch("--max-wait", "2")
        finally:
            timer.join()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["kind"], "mailbox_ready")
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_bounded_watch_refuses_foreign_owner_and_invalid_options(self):
        self.pending[0]["delivery_owner"]["provider"] = "kimi-code"
        self.save()
        result = self.bounded_watch()
        self.assertEqual(result.returncode, 3)
        self.assertIn("foreign pending delivery owner", result.stderr)
        for extra in [("--max-wait", "0"), ("--max-wait", "nan"),
                      ("--max-wait", "61"), ("--once",), ("--read-pending",),
                      ("--ack", self.ids[0]), ("--to", "leon"), ("--takeover",)]:
            with self.subTest(extra=extra):
                self.assertEqual(self.bounded_watch(*extra).returncode, 2)

    def open_watch(self, timeout):
        """--until-event without --max-wait: no deadline, so no empty turn."""
        return subprocess.run(
            [sys.executable, str(SOURCE), "--watch", "--until-event",
             "--provider", "codex", "--session", self.session,
             "--bridge-home", str(self.root)],
            capture_output=True, text=True, timeout=timeout)

    def test_open_watch_never_ends_on_an_empty_mailbox(self):
        self.ack(self.ids)
        self.save()
        with self.assertRaises(subprocess.TimeoutExpired) as caught:
            self.open_watch(timeout=1.5)
        self.assertFalse((caught.exception.stdout or b"").strip())
        self.assertFalse((self.root / "acknowledgments" / self.lease / "watch_timeout").exists())

    def test_open_watch_ends_only_when_a_message_arrives(self):
        import threading
        self.pending = []
        self.save()
        def arrive():
            self.pending = [self.envelope(self.ids[0])]
            self.save()
        timer = threading.Timer(0.4, arrive)
        timer.start()
        try:
            result = self.open_watch(timeout=5)
        finally:
            timer.join()
        self.assertEqual(result.returncode, 0, result.stderr)
        notice = json.loads(result.stdout)
        self.assertEqual(notice["kind"], "mailbox_ready")
        self.assertEqual(notice["pending_count"], 1)
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_bounded_watch_refuses_missing_lease_without_creating_one(self):
        (self.root / "leases" / f"{self.lease}.json").unlink()
        result = self.bounded_watch()
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertFalse((self.root / "leases" / f"{self.lease}.json").exists())

    def test_archive_and_mailbox_read_cannot_combine_before_any_state_change(self):
        before = {str(p.relative_to(self.root)): p.read_bytes()
                  for p in self.root.rglob("*") if p.is_file()}
        for extra in [
                ["--archive-agent", "3", "--lease", self.lease, "--read-pending"],
                ["--archive-agent", "3", "--lease", self.lease, "--read-limit", "1"],
                ["--archive-agent", "3", "--lease", self.lease, "--read-bytes", "1000"]]:
            with self.subTest(extra=extra):
                result = subprocess.run([
                    sys.executable, str(SOURCE), "--provider", "codex", "--session", self.session,
                    "--bridge-home", str(self.root), "--bus", str(self.bus), *extra],
                    text=True, capture_output=True, check=False)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertEqual(before, {str(p.relative_to(self.root)): p.read_bytes()
                                         for p in self.root.rglob("*") if p.is_file()})

    def test_five_equal_messages_are_read_completely_without_automatic_ack(self):
        before = (self.root / "leases" / f"{self.lease}.json").read_bytes()
        result = self.read()
        self.assertEqual(result["deliveries"], self.pending)
        self.assertEqual(result["read_delivery_ids"], self.ids)
        self.assertEqual(result["remaining"], 0)
        self.assertEqual(before, (self.root / "leases" / f"{self.lease}.json").read_bytes())
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_batch_ack_settles_each_native_submission_then_stale_bell_reads_empty(self):
        for n, identity in enumerate(self.ids):
            submission = f"aaaaaaaa-bbbb-4ccc-8ddd-{n:012x}"
            DEMUX.atomic_json(self.root / "wakeups" / self.lease / f"{identity}.json", {
                "schema": "codescribe.native-queue.receipt.v1", "lease_id": self.lease,
                "provider": "codex", "provider_session_id": self.session, "delivery_id": identity,
                "disposition": "provider_accepted",
                "provider_receipt": f"Queued message {submission} for thread {self.session}.",
            })
        read = self.read()
        self.ack(read["read_delivery_ids"])
        self.assertEqual(self.read()["deliveries"], [])
        for identity in self.ids:
            self.assertEqual(DEMUX.read_json(self.root / "wakeups" / self.lease / f"{identity}.json")["queue_disposition"], "removed")

    def test_ack_records_read_time_once_and_redacts_nested_transcript(self):
        self.pending[0]["occurrences"] = [{"sample_start": 10, "sample_end": 20, "label": "secret-transcript-specimen"}]
        self.save()
        self.ack([self.ids[0]])
        marker = self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json"
        before = marker.read_bytes()
        receipt = json.loads(before)
        self.assertTrue(receipt["read_at"])
        self.assertNotIn("secret-transcript-specimen", before.decode())
        self.assertNotIn('"text"', before.decode())
        self.ack([self.ids[0]])
        self.assertEqual(marker.read_bytes(), before)

    def test_arrival_between_read_and_ack_remains_unread(self):
        read = self.read(read_limit=2)
        extra = self.envelope("f" * 24)
        self.pending.append(extra)
        self.save()
        self.ack(read["read_delivery_ids"])
        self.assertEqual(self.read()["read_delivery_ids"], self.ids[2:] + ["f" * 24])

    def test_drafts_do_not_hold_terminal_drain_open(self):
        self.pending.insert(0, dict(self.envelope("e" * 24), kind="revised"))
        self.save()
        self.assertEqual(self.read()["read_delivery_ids"], self.ids)
        self.ack(self.ids)
        self.assertEqual(self.read()["remaining"], 0)

    def test_routing_notice_can_be_read_and_acknowledged_without_becoming_a_command(self):
        notice = dict(self.envelope("e" * 24), kind="routing_ambiguity", state_change_allowed=False)
        self.pending = [notice]
        self.save()
        read = self.read()
        self.assertEqual(read["deliveries"], [notice])
        self.ack(read["read_delivery_ids"])
        self.assertEqual(self.read()["remaining"], 0)

    def test_limit_does_not_truncate_or_ack_unread_items(self):
        result = self.read(read_limit=2)
        self.assertEqual(result["deliveries"], self.pending[:2])
        self.assertEqual(result["remaining"], 3)
        self.ack(result["read_delivery_ids"])
        self.assertEqual(self.read()["deliveries"], self.pending[2:])

    def test_byte_limit_refuses_oversized_first_envelope_without_partial_text(self):
        self.pending[0]["text"] = "Iwo " * 20000
        self.save()
        with patch.object(DEMUX, "emit") as emit, self.assertRaisesRegex(ValueError, "read-bytes"):
            DEMUX.read_pending_command(self.args())
        emit.assert_not_called()
        self.assertEqual(self.read(read_bytes=200000)["deliveries"], self.pending)

    def test_foreign_payload_refuses_whole_read(self):
        self.pending[3]["provider_session_id"] = "foreign"
        self.save()
        with patch.object(DEMUX, "emit") as emit, self.assertRaises(ValueError):
            DEMUX.read_pending_command(self.args())
        emit.assert_not_called()

    def test_foreign_nested_owner_refuses_whole_read(self):
        self.pending[2]["delivery_owner"] = dict(self.pending[2]["delivery_owner"], provider="foreign")
        self.save()
        with patch.object(DEMUX, "emit") as emit, self.assertRaises(ValueError):
            DEMUX.read_pending_command(self.args())
        emit.assert_not_called()

    def test_acknowledged_delivery_cannot_be_read_again_before_follower_sweep(self):
        self.ack([self.ids[0]])
        result = subprocess.run([sys.executable, str(SOURCE), "--read-delivery", self.ids[0],
                                 "--provider", "codex", "--session", self.session,
                                 "--bridge-home", str(self.root)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    def test_invalid_read_limits_and_foreign_lease_refuse_without_output(self):
        for kwargs in ({"read_limit": 0}, {"read_limit": 257}, {"read_bytes": 0},
                       {"read_bytes": 16 * 1024 * 1024 + 1}):
            with self.subTest(kwargs=kwargs), patch.object(DEMUX, "emit") as emit, self.assertRaises(ValueError):
                args = self.args()
                for k, v in kwargs.items():
                    setattr(args, k, v)
                DEMUX.read_pending_command(args)
            emit.assert_not_called()
        path = self.root / "leases" / f"{self.lease}.json"
        state = DEMUX.read_json(path)
        state["provider_session_id"] = "foreign"
        DEMUX.atomic_json(path, state)
        with patch.object(DEMUX, "emit") as emit, self.assertRaises(ValueError):
            DEMUX.read_pending_command(self.args())
        emit.assert_not_called()

    def test_five_pcm_entries_survive_full_read_and_ack(self):
        row = self.pending[0]
        row.update(kind="seal", text="Iwo Iwo Iwo Iwo Iwo", occurrences=[{
            "occurrence_session_id": "capture", "capture_epoch": 1,
            "sample_start": n * 1600, "sample_end": (n + 1) * 1600,
            "document_index": 0, "label": "Iwo", "receipt_id": str(n),
        } for n in range(5)])
        self.pending = [row]
        self.save()
        read = self.read()
        self.assertEqual(read["deliveries"][0]["text"], row["text"])
        self.assertNotIn("occurrences", read["deliveries"][0])
        raw = subprocess.run([sys.executable, str(SOURCE), "--read-delivery", self.ids[0],
                              "--provider", "codex", "--session", self.session,
                              "--bridge-home", str(self.root)], capture_output=True, text=True)
        self.assertEqual(raw.returncode, 0, raw.stderr)
        self.assertEqual(json.loads(raw.stdout), row)
        self.ack(read["read_delivery_ids"])
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json")
        self.assertEqual(len(marker["envelope"]["occurrences"]), 5)
        self.assertEqual(len({o["sample_start"] for o in marker["envelope"]["occurrences"]}), 5)

    def test_native_message_contains_full_task_and_read_then_ack_for_current_ownership(self):
        self.pending[0]["text"] = "Do not install old build 2094; this text stays in the bus."
        self.save()
        with patch("shutil.which", return_value="/fake/codex"), patch("subprocess.run",
                return_value=subprocess.CompletedProcess([], 0, "queued", "")) as run:
            wake = DEMUX.NativeQueueWakeup(self.root, self.session, "2")
            wake.enqueue(self.pending[0])
            wake.close(wait=True)
        notice = run.call_args.args[0][5]
        self.assertIn(self.pending[0]["text"], notice)
        self.assertIn("--read-pending", notice)
        self.assertIn("--ack", notice)
        self.assertIn(self.ids[0], notice)
        self.assertLess(len(notice) - len(self.pending[0]["text"]), 220)
        self.assertNotIn("Delivery provenance", notice)
        # Full owner/timestamp coordinates remain in the authoritative read,
        # rather than repeating the envelope around every queue copy.
        received = self.read()["deliveries"][0]
        self.assertEqual(received["provider_session_id"], self.session)
        self.assertEqual(received["emitted_at"], "2026-10-07T13:37:33Z")
        self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease, self.ids[0]))

    def test_short_message_with_large_pcm_diagnostics_fits_default_read_budget(self):
        text = "Astra, odpowiedz od razu.\nPięć Iwo: Iwo Iwo Iwo Iwo Iwo. `echo $HOME` i $(touch x) to tekst."
        row = self.pending[0]
        row.update(kind="seal", text=text, wav="/private/audio.wav", occurrences=[{
            "occurrence_session_id": "capture", "capture_epoch": 1,
            "sample_start": n * 1600, "sample_end": (n + 1) * 1600,
            "document_index": 0, "label": "Iwo", "receipt_id": str(n),
            "acoustic_receipts": [{"diagnostic": "x" * 32768}],
        } for n in range(5)])
        self.pending = [row]
        self.save()
        path = self.root / "leases" / f"{self.lease}.json"
        before = path.read_bytes()
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(len(result.stdout.encode("utf-8")), 8192)
        read = json.loads(result.stdout)
        self.assertEqual(read["read_delivery_ids"], [self.ids[0]])
        self.assertEqual(read["deliveries"][0]["text"], text)
        self.assertEqual(read["deliveries"][0]["delivery_owner"], row["delivery_owner"])
        self.assertNotIn("occurrences", read["deliveries"][0])
        self.assertNotIn("/private/audio.wav", result.stdout)
        self.assertEqual(path.read_bytes(), before)
        raw = subprocess.run([sys.executable, str(SOURCE), "--read-delivery", self.ids[0],
                              "--provider", "codex", "--session", self.session,
                              "--bridge-home", str(self.root)], capture_output=True, text=True)
        self.assertEqual(raw.returncode, 0, raw.stderr)
        self.assertEqual(json.loads(raw.stdout), row)
        self.ack(read["read_delivery_ids"])
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json")
        self.assertEqual([o["sample_start"] for o in marker["envelope"]["occurrences"]],
                         [n * 1600 for n in range(5)])
        self.assertEqual(self.read()["deliveries"], [])

    def test_peer_message_keeps_complete_text_and_provenance_without_audio_diagnostics(self):
        row = self.pending[0]
        row.update(source="agent", sender={"name": "lena", "provider": "codex", "provider_session_id": "peer"},
                   peer_to="astra", association="addressed", spoken=False,
                   reply_id="a" * 24, reply_to="b" * 24,
                   producer_schema="codescribe.agent-message.v1", source_event_id="c" * 24,
                   text="Raport agenta, nie nowa dyspozycja Foundera.\nZachowaj zakres i autorstwo.",
                   occurrences=[{"acoustic_receipts": ["diagnostic"]}])
        self.pending = [row]
        self.save()
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        compact = json.loads(result.stdout)["deliveries"][0]
        for field in ("text", "source", "sender", "peer_to", "association", "spoken", "reply_id", "reply_to",
                      "producer_schema", "source_event_id", "emitted_at", "delivery_owner"):
            self.assertEqual(compact[field], row[field], field)
        self.assertNotIn("occurrences", compact)

    def cli(self, *options):
        return subprocess.run([sys.executable, str(SOURCE), "--read-pending", "--provider", "codex",
                               "--session", self.session, "--bridge-home", str(self.root),
                               "--bus", str(self.bus), *options], capture_output=True, text=True)

    def test_cli_full_read_then_ack_can_be_followed_by_empty_read(self):
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        read = json.loads(result.stdout)
        self.assertEqual(read["deliveries"], self.pending)
        ack = subprocess.run([sys.executable, str(SOURCE), "--provider", "codex", "--session", self.session,
                              "--bridge-home", str(self.root), "--bus", str(self.bus),
                              "--ack", *read["read_delivery_ids"]], capture_output=True, text=True)
        self.assertEqual(ack.returncode, 0, ack.stderr)
        receipts = [json.loads(line) for line in ack.stdout.splitlines()]
        self.assertEqual([r["delivery_id"] for r in receipts], self.ids)
        self.assertTrue(all(r["native_queue_settled"] for r in receipts))
        for identity in self.ids:
            self.assertTrue(DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{identity}.json")["read_at"])
        self.assertEqual(json.loads(self.cli().stdout)["deliveries"], [])

    def test_read_never_combines_with_mutation(self):
        for option in ("--detach", "--attach", "--ack", "--version", "--status", "--send"):
            extra = [option, self.ids[0]] if option in ("--ack", "--send") else [option]
            result = self.cli(*extra)
            self.assertNotEqual(result.returncode, 0, option)
            self.assertEqual(result.stdout, "", option)


if __name__ == "__main__":
    unittest.main()
