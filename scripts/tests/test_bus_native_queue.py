"""Native queue acceptance is separate from conversation acknowledgment."""
import argparse
import threading
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bus_native_queue_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class NativeQueueTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.session = "native-test-thread"
        self.lease_id = DEMUX.lease_identifier("codex", self.session)
        self.ids = [f"{index:024x}" for index in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id,
            "provider": "codex", "provider_session_id": self.session,
            "pending": self.pending,
        })

    def envelope(self, identity):
        return {
            "kind": "seal", "delivery_id": identity, "lease_id": self.lease_id,
            "provider": "codex", "provider_session_id": self.session,
            "delivery_owner": {"lease_id": self.lease_id, "provider": "codex",
                               "provider_session_id": self.session},
            "audience": "lena", "text": "Lena, pięć Iwo; `echo $HOME` nie jest kodem.",
            "state_change_allowed": False, "coverage": "refused",
            "wav": "/private/audio.wav", "sample_start": 123, "sample_end": 456,
        }

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, "queued", ""))
    def test_typed_message_uses_native_queue_without_audio_seal(self, run, _which):
        payload = dict(self.pending[0], kind="message", source="typed", state_change_allowed=True)
        for key in ("coverage", "wav", "sample_start", "sample_end"): payload.pop(key, None)
        self.pending[0] = payload
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        state["pending"] = self.pending
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", state)
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(self.result(payload["delivery_id"])["disposition"], "provider_accepted")
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 1)

    def wake(self):
        return DEMUX.NativeQueueWakeup(self.root, self.session, "2")

    def result(self, identity):
        return DEMUX.read_json(self.root / "wakeups" / self.lease_id / f"{identity}.json")

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, '{"queued":true}', ""))
    def test_five_occurrences_ordered_once_compact_and_not_acknowledged(self, run, _which):
        wake = self.wake()
        for payload in self.pending:
            wake.enqueue(payload)
            wake.enqueue(payload)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 5)
        for call, identity in zip(run.call_args_list, self.ids):
            argv = call.args[0]
            self.assertEqual(argv[:5], ["/fake/codex", "queue", "--thread", self.session, "--message"])
            message = argv[5]
            self.assertIn(self.pending[0]["text"], message)
            self.assertIn("--read-pending", message)
            self.assertIn("--ack", message)
            self.assertIn(identity, message)
            self.assertLess(len(message) - len(self.pending[0]["text"]), 220)
            self.assertNotIn("Delivery provenance", message)
            self.assertNotIn("read_delivery_ids", message)
            self.assertNotIn("sample_start", message)
            self.assertNotIn("/private/audio.wav", message)
            self.assertNotIn("shell", call.kwargs)
            self.assertEqual(self.result(identity)["disposition"], "provider_accepted")
            self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, identity))
        resumed = self.wake()
        for payload in self.pending:
            resumed.enqueue(payload)
        resumed.close(wait=True)
        self.assertEqual(run.call_count, 5)
        state = DEMUX.read_json(self.root / "leases" / f"{self.lease_id}.json")
        self.assertEqual(state["pending"], self.pending)

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", side_effect=subprocess.TimeoutExpired("codex", 30))
    def test_timeout_is_uncertain_and_restart_does_not_replay(self, run, _which):
        for _ in range(2):
            wake = self.wake()
            wake.enqueue(self.pending[0])
            wake.close(wait=True)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(self.result(self.ids[0])["disposition"], "uncertain")

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 1, "", "server draining"))
    def test_rejection_retains_delivery_and_explicit_retry(self, run, _which):
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        self.assertEqual(self.result(self.ids[0])["disposition"], "rejected")
        run.return_value = subprocess.CompletedProcess([], 0, "queued", "")
        wake = self.wake()
        wake.enqueue(self.pending[0], retry=True)
        wake.close(wait=True)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(self.result(self.ids[0])["disposition"], "provider_accepted")
        self.assertEqual(self.result(self.ids[0])["attempt"], 2)

    @patch("subprocess.run")
    def test_foreign_owner_draft_missing_and_acknowledged_never_queue(self, run):
        invalid = [dict(self.pending[0], kind="draft"),
                   dict(self.pending[0], provider_session_id="other"),
                   dict(self.pending[0], delivery_owner={"provider_session_id": "other"}),
                   self.envelope("f" * 24)]
        DEMUX.atomic_json(self.root / "acknowledgments" / self.lease_id / f"{self.ids[1]}.json", {
            "lease_id": self.lease_id, "delivery_id": self.ids[1],
        })
        wake = self.wake()
        for payload in invalid + [self.pending[1]]:
            wake.enqueue(payload)
        wake.close(wait=True)
        run.assert_not_called()

    def test_attached_follower_wakes_without_blocking_bus_and_restores_same_cursor(self):
        bindir = self.root / "bin"
        bindir.mkdir()
        record = self.root / "provider-input.jsonl"
        executable = bindir / "codex"
        executable.write_text(
            f"#!{sys.executable}\nimport json,sys,time\n"
            f"with open({str(record)!r}, 'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n"
            "time.sleep(0.3)\nprint('queued')\n"
        )
        executable.chmod(0o755)
        bus = self.root / "channel.jsonl"
        bus.touch()
        environment = dict(os.environ, PATH=str(bindir) + os.pathsep + os.environ["PATH"])
        command = [sys.executable, str(SPEC.origin), "--bus", str(bus),
                   "--bridge-home", str(self.root / "attached"), "--attach",
                   "--channel", "2", "--name", "lena", "--provider", "codex",
                   "--session", "test-attached-thread"]
        receipt = json.loads(subprocess.check_output(command, env=environment, text=True))
        pid = receipt["follower_pid"]
        try:
            self.assertEqual(receipt["wakeup"], "codex-queue")
            with bus.open("a") as output:
                for index in range(5):
                    output.write(json.dumps({
                        "schema": DEMUX.CLEAN_SCHEMA, "session_id": f"take-{index}",
                        "utterance_id": f"utterance-{index}", "sequence": index,
                        "status": "transcript_sealed", "audience": "lena",
                        "text": "Lena, Iwo.",
                    }) + "\n")
            lease_file = self.root / "attached" / "leases" / f"{receipt['lease_id']}.json"
            deadline = time.monotonic() + 7
            while time.monotonic() < deadline:
                state = DEMUX.read_json(lease_file) or {}
                if len(state.get("pending", [])) == 5 and state.get("cursor") == bus.stat().st_size:
                    break
                time.sleep(0.02)
            self.assertEqual(len(state.get("pending", [])), 5)
            self.assertEqual(state["cursor"], bus.stat().st_size)
            while time.monotonic() < deadline:
                rows = record.read_text().splitlines() if record.exists() else []
                receipts = list((self.root / "attached" / "wakeups" / receipt["lease_id"]).glob("*.json"))
                if len(receipts) == 5 and all(DEMUX.read_json(p).get("disposition") == "provider_accepted" for p in receipts):
                    break
                time.sleep(0.02)
            self.assertEqual(len(rows), 5)
            self.assertEqual([json.loads(row)[2] for row in rows], ["test-attached-thread"] * 5)
            # Changing to monitor-only updates the actual follower, preserving unread occurrences.
            updated = json.loads(subprocess.check_output(command + ["--wakeup", "off"], env=environment, text=True))
            pid = updated["follower_pid"]
            self.assertEqual(updated["wakeup"], "off")
            self.assertNotEqual(pid, receipt["follower_pid"])
            self.assertEqual(updated["cursor"], state["cursor"])
            self.assertEqual(len(DEMUX.read_json(lease_file)["pending"]), 5)
            again = json.loads(subprocess.check_output(command + ["--wakeup", "off"], env=environment, text=True))
            self.assertFalse(again["follower_spawned"])
            self.assertEqual(again["follower_pid"], pid)
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass

    def test_busy_turn_bell_carries_complete_text_and_owner_without_ack(self):
        text = "Zażółć gęślą jaźń.\n" * 90 + "KONIEC PEŁNEJ WIADOMOŚCI"
        event = dict(self.pending[0], schema=DEMUX.EVENT_SCHEMA, status="transcript_sealed",
                     text=text, sender={"name": "peer", "provider": "claude-code"},
                     association={"reply_to": "original"})
        events = self.root / "notifications.jsonl"
        events.write_text(json.dumps(event) + "\n")
        bell = subprocess.check_output([
            sys.executable, SPEC.origin, "--watch", "--once", "--from-file", str(events),
        ], text=True)
        received_bell = json.loads(bell)
        self.assertEqual(received_bell["delivery_id"], event["delivery_id"])
        self.assertEqual(received_bell["text"], text)
        self.assertEqual(received_bell["lease_id"], self.lease_id)
        self.assertEqual(received_bell["sender"], event["sender"])
        self.assertEqual(received_bell["association"], event["association"])
        self.assertNotIn("wav", received_bell)
        explicit_bell = subprocess.check_output([
            sys.executable, SPEC.origin, "--watch", "--bell", "--once", "--from-file", str(events),
        ], text=True)
        self.assertEqual(json.loads(explicit_bell), received_bell)
        full = subprocess.check_output([
            sys.executable, SPEC.origin, "--watch", "--full", "--once", "--from-file", str(events),
        ], text=True)
        self.assertEqual(json.loads(full)["text"], event["text"])
        received = subprocess.check_output([
            sys.executable, SPEC.origin, "--read-delivery", event["delivery_id"],
            "--provider", "codex", "--session", self.session, "--bridge-home", str(self.root),
        ], text=True)
        self.assertEqual(json.loads(received), self.pending[0])
        self.assertFalse(DEMUX.delivery_acknowledged(self.root, self.lease_id, event["delivery_id"]))

    @patch("shutil.which", return_value="/fake/codex")
    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 0, "queued", ""))
    def test_queue_preserves_long_multiline_message_with_bounded_wrapper(self, run, _which):
        text = "Pierwszy akapit.\n\n" + "Iwo żółć. " * 700 + "\nOSTATNIE SŁOWO"
        self.pending[0]["text"] = text
        state_path = self.root / "leases" / f"{self.lease_id}.json"
        state = DEMUX.read_json(state_path)
        state["pending"] = self.pending
        DEMUX.atomic_json(state_path, state)
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        message = run.call_args.args[0][5]
        self.assertIn(text, message)
        self.assertLess(len(message) - len(text), 220)
        self.assertNotIn("Delivery provenance", message)

    def test_watch_exits_quietly_when_its_consumer_closes_the_pipe(self):
        """``--watch | head -1`` is an exit-on-bell wakeup, not a crash."""
        events = self.root / "notifications.jsonl"
        rows = [dict(self.envelope(identity), schema=DEMUX.EVENT_SCHEMA,
                     status="transcript_sealed", text=f"Iwo {identity}")
                for identity in self.ids[:2]]
        events.write_text(json.dumps(rows[0]) + "\n")
        process = subprocess.Popen(
            [sys.executable, SPEC.origin, "--watch", "--from-start", "--from-file", str(events)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            first = process.stdout.readline()
            self.assertEqual(json.loads(first)["delivery_id"], rows[0]["delivery_id"])
            process.stdout.close()
            with events.open("a") as handle:
                handle.write(json.dumps(rows[1]) + "\n")
            stderr = process.communicate(timeout=10)[1]
        finally:
            if process.poll() is None:
                process.kill()
        self.assertEqual(process.returncode, 0, stderr)
        self.assertNotIn("Traceback", stderr)
        self.assertNotIn("BrokenPipeError", stderr)

    def test_twenty_complete_bells_ack_and_withdraw_before_work_without_replay(self):
        self.ids = [f"{index:024x}" for index in range(1, 21)]
        self.pending = [dict(self.envelope(identity), schema=DEMUX.EVENT_SCHEMA,
                             status="transcript_sealed", text="Iwo " * 180 + str(index))
                        for index, identity in enumerate(self.ids)]
        state_path = self.root / "leases" / f"{self.lease_id}.json"
        state = DEMUX.read_json(state_path)
        state["pending"] = self.pending
        DEMUX.atomic_json(state_path, state)
        self.prepare_ack_state()
        events = self.root / "notifications.jsonl"
        events.write_text("".join(json.dumps(p) + "\n" for p in self.pending))
        command = [sys.executable, SPEC.origin, "--watch", "--once", "--from-file", str(events),
                   "--provider", "codex", "--session", self.session, "--bridge-home", str(self.root)]
        notifications = [json.loads(line) for line in subprocess.check_output(command, text=True).splitlines()]
        self.assertEqual([row["text"] for row in notifications], [p["text"] for p in self.pending])
        submissions = []
        for index, identity in enumerate(self.ids):
            self.accepted_receipt(identity)
            path = self.root / "wakeups" / self.lease_id / f"{identity}.json"
            receipt = self.result(identity)
            submission = f"11111111-2222-4333-8444-{index:012x}"
            receipt["provider_receipt"] = f"Queued message {submission} for thread {self.session}."
            DEMUX.atomic_json(path, receipt)
            submissions.append(submission)
        with patch.object(DEMUX, "delete_native_queue_submission", return_value=True) as delete:
            for notification in notifications:
                DEMUX.acknowledge_delivery(self.ack_args(notification["delivery_id"]))
                self.assertEqual(self.result(notification["delivery_id"])["queue_disposition"], "removed")
        self.assertEqual([call.args[2] for call in delete.call_args_list], submissions)
        self.assertEqual(subprocess.check_output(command, text=True), "")
        self.assertEqual(DEMUX.read_json(state_path)["pending"], self.pending)

    def ack_args(self, identity):
        return argparse.Namespace(ack=[identity], provider="codex", session=self.session,
                                  bridge_home=self.root, bus=self.root / "bus.jsonl",
                                  bus_overridden=False)

    def accepted_receipt(self, identity):
        submission = "11111111-2222-4333-8444-555555555555"
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{identity}.json", {
            "schema": "codescribe.native-queue.receipt.v1", "lease_id": self.lease_id,
            "provider": "codex", "provider_session_id": self.session,
            "delivery_id": identity, "disposition": "provider_accepted",
            "provider_receipt": f"Queued message {submission} for thread {self.session}.",
        })
        return submission

    def prepare_ack_state(self):
        path = self.root / "leases" / f"{self.lease_id}.json"
        state = DEMUX.read_json(path)
        state["bus"] = str(self.root / "bus.jsonl")
        DEMUX.atomic_json(path, state)

    def test_ack_withdraws_exact_owned_submission_and_is_idempotent(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        submission = self.accepted_receipt(identity)
        with patch.object(DEMUX, "delete_native_queue_submission", return_value=True) as delete:
            DEMUX.acknowledge_delivery(self.ack_args(identity))
            DEMUX.acknowledge_delivery(self.ack_args(identity))
        delete.assert_called_once_with(self.root, self.session, submission)
        self.assertEqual(self.result(identity)["queue_disposition"], "removed")
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, identity))
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease_id / f"{identity}.json")
        self.assertEqual(marker["envelope"]["sample_end"], 456)

    def test_foreign_receipt_and_unread_delivery_never_delete(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        self.accepted_receipt(identity)
        with patch.object(DEMUX, "delete_native_queue_submission") as delete:
            self.assertFalse(DEMUX.withdraw_acknowledged_queue(self.root, self.lease_id, identity))
            receipt = self.result(identity)
            receipt["provider_session_id"] = "foreign"
            DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{identity}.json", receipt)
            DEMUX.acknowledge_delivery(self.ack_args(identity))
        delete.assert_not_called()

    def test_failed_delete_retains_ack_and_retries_without_resending(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        self.accepted_receipt(identity)
        with patch.object(DEMUX, "delete_native_queue_submission", side_effect=ValueError("unavailable")):
            DEMUX.acknowledge_delivery(self.ack_args(identity))
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, identity))
        self.assertEqual(self.result(identity)["queue_disposition"], "pending")
        with patch.object(DEMUX, "delete_native_queue_submission", return_value=False):
            self.assertTrue(DEMUX.withdraw_acknowledged_queue(self.root, self.lease_id, identity, retry=True))
        self.assertEqual(self.result(identity)["queue_disposition"], "not_pending")

    def test_ack_during_provider_submission_withdraws_after_acceptance(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        entered, release = threading.Event(), threading.Event()
        submission = "11111111-2222-4333-8444-555555555555"
        def send(*args, **kwargs):
            entered.set()
            self.assertTrue(release.wait(3))
            return subprocess.CompletedProcess([], 0, f"Queued message {submission} for thread {self.session}.", "")
        with patch("shutil.which", return_value="/fake/codex"), patch("subprocess.run", side_effect=send), \
                patch.object(DEMUX, "delete_native_queue_submission", return_value=True) as delete:
            wake = self.wake()
            wake.enqueue(self.pending[0])
            try:
                self.assertTrue(entered.wait(3))
                DEMUX.acknowledge_delivery(self.ack_args(identity))
                delete.assert_not_called()
            finally:
                release.set()
                wake.close(wait=True)
        delete.assert_called_once_with(self.root, self.session, submission)
        self.assertEqual(self.result(identity)["queue_disposition"], "removed")

    def test_mismatched_provider_receipt_cannot_claim_withdrawal(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        self.accepted_receipt(identity)
        path = self.root / "wakeups" / self.lease_id / f"{identity}.json"
        receipt = self.result(identity)
        receipt["provider_receipt"] = receipt["provider_receipt"].replace(self.session, "foreign")
        DEMUX.atomic_json(path, receipt)
        with patch.object(DEMUX, "delete_native_queue_submission") as delete:
            DEMUX.acknowledge_delivery(self.ack_args(identity))
        delete.assert_not_called()
        self.assertEqual(self.result(identity)["queue_disposition"], "unresolved")

    def test_failed_withdrawal_reuses_nonblocking_follower_executor(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        self.accepted_receipt(identity)
        DEMUX.atomic_json(self.root / "acknowledgments" / self.lease_id / f"{identity}.json",
                          {"lease_id": self.lease_id, "delivery_id": identity})
        entered, release = threading.Event(), threading.Event()
        def delete(*args):
            entered.set()
            self.assertTrue(release.wait(3))
            return True
        wake = self.wake()
        with patch.object(DEMUX, "delete_native_queue_submission", side_effect=delete) as call:
            try:
                wake.enqueue_withdrawal(identity)
                self.assertTrue(entered.wait(3))
                wake.enqueue_withdrawal(identity)
                self.assertEqual(call.call_count, 1)
            finally:
                release.set()
                wake.close(wait=True)
        self.assertEqual(self.result(identity)["queue_disposition"], "removed")

    def test_ack_before_sender_starts_suppresses_provider_queue(self):
        self.prepare_ack_state()
        DEMUX.acknowledge_delivery(self.ack_args(self.ids[0]))
        with patch("subprocess.run") as send:
            wake = self.wake()
            wake.enqueue(self.pending[0])
            wake.close(wait=True)
        send.assert_not_called()

    def test_five_physical_deliveries_with_same_text_withdraw_separately(self):
        for index, payload in enumerate(self.pending):
            payload.update(occurrence_session_id="five-iwo-capture", capture_epoch=1,
                           sample_start=index * 1000, sample_end=(index + 1) * 1000)
        path = self.root / "leases" / f"{self.lease_id}.json"
        state = DEMUX.read_json(path)
        state["pending"] = self.pending
        DEMUX.atomic_json(path, state)
        self.prepare_ack_state()
        submissions = [f"11111111-2222-4333-8444-{index:012x}" for index in range(1, 6)]
        for identity, submission in zip(self.ids, submissions):
            self.accepted_receipt(identity)
            path = self.root / "wakeups" / self.lease_id / f"{identity}.json"
            receipt = self.result(identity)
            receipt["provider_receipt"] = f"Queued message {submission} for thread {self.session}."
            DEMUX.atomic_json(path, receipt)
        args = self.ack_args(self.ids[0])
        args.ack = self.ids
        with patch.object(DEMUX, "delete_native_queue_submission", return_value=True) as delete:
            DEMUX.acknowledge_delivery(args)
        self.assertEqual([call.args[2] for call in delete.call_args_list], submissions)
        self.assertEqual(sum(self.result(identity)["queue_disposition"] == "removed" for identity in self.ids), 5)
        ranges = [DEMUX.read_json(self.root / "acknowledgments" / self.lease_id / f"{identity}.json")["envelope"] for identity in self.ids]
        self.assertEqual([(row["sample_start"], row["sample_end"]) for row in ranges],
                         [(index * 1000, (index + 1) * 1000) for index in range(5)])

    def test_public_help_names_installed_command_and_default_bell(self):
        help_text = subprocess.check_output([sys.executable, SPEC.origin, "--help"], text=True)
        self.assertTrue(help_text.startswith("usage: cs-bus"), help_text)
        self.assertNotIn("bus-demux.py", help_text)
        self.assertIn("default", help_text)
        self.assertIn("--full", help_text)


if __name__ == "__main__":
    unittest.main()
