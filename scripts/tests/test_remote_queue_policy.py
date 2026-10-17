"""Remote relay policy, durable ACK recovery and current-mailbox admission."""
import argparse
import contextlib
import io
import json
import os
import subprocess
import threading
import unittest
import unittest.mock as mock

if __package__:
    from . import test_remote_native_queue as base
else:
    import test_remote_native_queue as base

D = base.D


class RemoteQueuePolicyTests(unittest.TestCase):
    def setUp(self):
        base.NativeRemoteQueueTests.setUp(self)
        self.root = self.root.resolve()
    accepted = base.NativeRemoteQueueTests.accepted
    wake = base.NativeRemoteQueueTests.wake
    receipt = base.NativeRemoteQueueTests.receipt
    envelope = base.NativeRemoteQueueTests.envelope
    process = base.RemoteCodexTransportTests.process

    def test_follower_and_policy_writer_preserve_frames_across_short_writes(self):
        destination = self.root / "shared-events.jsonl"
        first_half, second_started, second_written = threading.Event(), threading.Event(), threading.Event()
        original_write = os.write
        shortened = False
        failures = []
        rows = [{"kind": "message", "text": "first frame"}, {"kind": "message", "text": "second frame"}]

        def short_write(fd, data):
            nonlocal shortened
            if threading.current_thread().name == "frame-A" and not shortened:
                shortened = True
                count = original_write(fd, data[:len(data) // 2])
                first_half.set()
                if not second_started.wait(2):
                    raise AssertionError("second writer did not start")
                # With serialization the second writer waits for A to finish;
                # without it B can append between A's two kernel writes.
                second_written.wait(0.25)
                return count
            count = original_write(fd, data)
            if threading.current_thread().name == "frame-B":
                second_written.set()
            return count

        def write(index):
            try:
                if index == 1:
                    if not first_half.wait(2):
                        raise AssertionError("first writer did not start")
                    second_started.set()
                D.emit_follower(rows[index], destination, None, human_output=False)
            except Exception as error:
                failures.append(error)

        with mock.patch.object(D.os, "write", side_effect=short_write):
            threads = [threading.Thread(target=write, args=(i,), name="frame-" + letter, daemon=True)
                       for i, letter in enumerate(("A", "B"))]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(3)
            self.assertFalse(any(thread.is_alive() for thread in threads))
        self.assertEqual(failures, [])
        self.assertEqual([json.loads(line) for line in destination.read_text().splitlines()], rows)

    def state(self, pending, *, bus=None):
        bus = bus or self.root / "bus-current.jsonl"
        row = {"schema": D.LEASE_SCHEMA, "lease_id": self.lease, "provider": "codex",
               "provider_session_id": self.session, "bus": str(bus), "pending": pending,
               "wakeup_configuration": {"wakeup": "off"}}
        D.atomic_json(self.root / "leases" / f"{self.lease}.json", row)
        return row

    def payload(self, index=0, *, channel="2", **extra):
        return dict(self.envelope(self.ids[index]), bus=str(self.root / "bus-current.jsonl"),
                    channel=channel, **extra)

    def policy(self, policy):
        bus = self.root / "bus-current.jsonl"
        D.atomic_json(D.broadcast_policy_path(self.root, "codex", self.session, bus), {
            "schema": D.BROADCAST_POLICY_SCHEMA, "provider": "codex", "provider_session_id": self.session,
            "lease_id": self.lease, "bus": str(bus), "policy": policy})

    def change_policy(self, policy):
        args = argparse.Namespace(bridge_home=self.root, provider="codex", session=self.session,
                                  bus=self.root / "bus-current.jsonl", bus_overridden=False, broadcast=policy)
        with mock.patch.object(D, "NativeQueueWakeup") as queue, mock.patch.object(D, "emit"):
            self.assertEqual(D.set_agent_broadcast_policy(args), 0)
        queue.assert_not_called()

    def watch(self, payloads):
        source = self.root / "history.jsonl"
        source.write_text("".join(json.dumps(row) + "\n" for row in payloads))
        args = argparse.Namespace(bridge_home=self.root, provider="codex", session=self.session,
                                  queue_relay=True, from_file=source, once=True, human=False, full=False)
        with mock.patch.object(D, "emit") as output:
            self.assertEqual(D.watch_command(args), 0)
        return [call.args[0] for call in output.call_args_list]

    def read(self, identity, *, admit):
        arguments = ["cs-bus", "--bridge-home", str(self.root), "--provider", "codex",
                     "--session", self.session, "--read-delivery", identity]
        if admit:
            arguments.append("--queue-admit")
        with mock.patch.object(D.sys, "argv", arguments), contextlib.redirect_stdout(io.StringIO()) as output, contextlib.redirect_stderr(io.StringIO()):
            try:
                code = D.main()
            except SystemExit as error:
                code = error.code
        return code, output.getvalue()

    def test_quiet_to_on_publishes_pending_to_existing_relay_without_remote_queue(self):
        pending = [self.payload(), self.payload(1, channel="0")]
        self.state(pending)
        self.policy("quiet")
        self.change_policy("on")
        events = D.follower_paths(self.root, self.lease)[1]
        rows = [json.loads(line) for line in events.read_text().splitlines()]
        self.assertEqual([row["delivery_id"] for row in rows], self.ids[:2])
        self.assertEqual([row["delivery_id"] for row in self.watch(rows)], self.ids[:2])
        self.assertFalse(list(self.root.rglob("wakeups/*.json")))
        self.assertFalse(list(self.root.rglob("acknowledgments/*.json")))

    def test_quiet_to_off_releases_direct_only_and_quiet_stays_silent(self):
        pending = [self.payload(), self.payload(1, channel="0")]
        self.state(pending)
        self.policy("quiet")
        self.change_policy("quiet")
        events = D.follower_paths(self.root, self.lease)[1]
        self.assertFalse(events.exists())
        self.change_policy("off")
        rows = [json.loads(line) for line in events.read_text().splitlines()]
        self.assertEqual([row["delivery_id"] for row in rows], self.ids[:1])

    def test_repeated_policy_command_repairs_interrupted_event_publication(self):
        payload = self.payload()
        self.state([payload])
        self.policy("on")  # policy persisted, but the command died before append
        self.change_policy("on")
        events = D.follower_paths(self.root, self.lease)[1]
        rows = [json.loads(line) for line in events.read_text().splitlines()]
        self.assertEqual([row["delivery_id"] for row in self.watch(rows)], self.ids[:1])

    def test_policy_release_includes_row_persisted_while_initial_snapshot_was_stale(self):
        self.state([])
        self.policy("quiet")
        payload = self.payload()
        write = D.atomic_json
        def arrival(path, value):
            if value.get("schema") == D.BROADCAST_POLICY_SCHEMA and value.get("policy") == "on":
                self.state([payload])
            write(path, value)
        with mock.patch.object(D, "atomic_json", side_effect=arrival):
            self.change_policy("on")
        events = D.follower_paths(self.root, self.lease)[1]
        self.assertEqual(json.loads(events.read_text())["delivery_id"], payload["delivery_id"])

    def test_explicit_remote_retry_requires_source_policy_but_plain_read_does_not(self):
        for channel, policy, admitted in [("0", "quiet", False), ("0", "off", False),
                                            ("2", "quiet", False), ("2", "off", True),
                                            ("0", "on", True), ("2", "on", True)]:
            with self.subTest(channel=channel, policy=policy):
                payload = self.payload(channel=channel)
                self.state([payload])
                self.policy(policy)
                code, words = self.read(payload["delivery_id"], admit=True)
                self.assertEqual(code == 0, admitted)
                self.assertEqual(bool(words), admitted)
                self.assertEqual(self.read(payload["delivery_id"], admit=False)[0], 0)

    def test_remote_retry_requests_source_admission_on_the_existing_ssh_operation(self):
        payload = self.payload()
        response = subprocess.CompletedProcess([], 0, json.dumps(payload), "")
        with mock.patch("subprocess.run", return_value=response) as ssh, mock.patch.object(D.NativeQueueWakeup, "enqueue"), mock.patch.object(D, "emit"):
            D.remote_codex_receipt_command("mailbox.test", ["--retry-wakeup", self.ids[0]], self.session, self.root, {})
        self.assertEqual(ssh.call_count, 1)
        self.assertIn("--queue-admit", ssh.call_args.args[0][-1])
        self.assertIn("--read-delivery " + self.ids[0], ssh.call_args.args[0][-1])

    def accepted_queue(self, identity):
        with mock.patch("shutil.which", return_value="/local/codex"), mock.patch("subprocess.run", side_effect=self.accepted):
            wake = self.wake()
            wake.enqueue(self.envelope(identity))
            wake.close(wait=True)

    def test_ack_control_does_not_hold_reader_behind_provider_withdrawal(self):
        self.accepted_queue(self.ids[0])
        control = {"kind": "queue_relay_ack", "provider": "codex", "provider_session_id": self.session,
                   "lease_id": self.lease, "delivery_id": self.ids[0]}
        entered, release, delivered = threading.Event(), threading.Event(), threading.Event()
        def delete(*args):
            entered.set()
            self.assertTrue(release.wait(2), "reader waited for the provider instead of forwarding the next bell")
            return True
        def present(row):
            if row.get("delivery_id") == self.ids[1]:
                delivered.set()
                release.set()
        with mock.patch("subprocess.Popen", return_value=self.process(control, self.payload(1))), mock.patch("shutil.which", return_value="/local/codex"), mock.patch("subprocess.run", side_effect=self.accepted), mock.patch.object(D, "delete_native_queue_submission", side_effect=delete), mock.patch.object(D, "emit", side_effect=present):
            try:
                self.assertEqual(D.remote_codex_watch("mailbox.test", ["--watch", "--bell"], self.session, self.root, {}), 0)
            finally:
                release.set()
        self.assertTrue(delivered.is_set())
        self.assertTrue(entered.is_set())
        self.assertEqual(self.receipt(self.ids[0])["queue_disposition"], "removed")

    def test_settled_replayed_ack_reuses_marker_without_fsync_or_executor_work(self):
        self.accepted_queue(self.ids[0])
        wake = self.wake()
        with mock.patch.object(D, "delete_native_queue_submission", return_value=True):
            self.assertTrue(wake.acknowledge_remote(self.ids[0]))
        with mock.patch.object(D, "atomic_json", wraps=D.atomic_json) as write, mock.patch.object(wake.executor, "submit") as submit:
            self.assertTrue(wake.acknowledge_remote(self.ids[0], wait=False))
            self.assertTrue(wake.acknowledge_remote(self.ids[0], wait=False))
        wake.close(wait=True)
        write.assert_not_called()
        submit.assert_not_called()

    def test_replay_rejects_old_bus_and_nonpending_current_delivery(self):
        old = dict(self.payload(), bus=str(self.root / "bus-old.jsonl"), text="completed old request")
        self.state([])
        self.assertEqual(self.watch([old, self.payload(1)]), [])

    def historical_ack(self, payload):
        owner = {key: payload[key] for key in ("provider", "provider_session_id", "lease_id", "bus")}
        D.atomic_json(self.root / "acknowledgments" / self.lease / f"{payload['delivery_id']}.json", {
            **owner, "delivery_id": payload["delivery_id"], "envelope": D.receipt_envelope(payload, payload["bus"])})

    def test_old_bus_ack_can_settle_queue_without_exposing_historical_words(self):
        old = dict(self.payload(), bus=str(self.root / "bus-old.jsonl"), text="completed old request")
        current = self.payload(1)
        self.state([current])
        self.historical_ack(old)
        rows = self.watch([old, current])
        self.assertEqual([row["kind"] for row in rows], ["queue_relay_ack", "message"])
        self.assertNotIn("text", rows[0])
        self.assertEqual(rows[1]["text"], current["text"])

    def test_historical_ack_must_match_the_actual_old_bus_envelope(self):
        old = dict(self.payload(), bus=str(self.root / "bus-old.jsonl"))
        self.state([])
        self.historical_ack(dict(old, bus=str(self.root / "unrelated-bus.jsonl")))
        self.assertEqual(self.watch([old]), [])

    def test_draft_and_revision_ids_never_enter_periodic_ack_checks(self):
        drafts = [self.payload(kind=kind) for kind in D.DRAFT_KINDS]
        terminal = self.payload(1)
        self.state(drafts + [terminal])
        source = self.root / "history.jsonl"
        source.write_text("".join(json.dumps(row) + "\n" for row in drafts + [terminal]))
        args = argparse.Namespace(bridge_home=self.root, provider="codex", session=self.session,
                                  queue_relay=True, from_file=source, once=False, human=False,
                                  full=False, from_start=True, interval=0.1)
        trigger = mock.Mock()
        trigger.wait.side_effect = [None, KeyboardInterrupt]
        with mock.patch.object(D, "BusEventTrigger", return_value=trigger), mock.patch.object(D, "delivery_acknowledged", wraps=D.delivery_acknowledged) as ack, mock.patch.object(D, "unread_pending") as read, mock.patch.object(D, "emit"):
            self.assertEqual(D.watch_command(args), 130)
        self.assertTrue(ack.call_args_list)
        self.assertTrue(all(call.args[2] == terminal["delivery_id"] for call in ack.call_args_list))
        read.assert_not_called()
        trigger.close.assert_called_once()

    def test_replayed_ack_recovers_failed_withdrawal_without_rewriting_marker(self):
        self.accepted_queue(self.ids[0])
        wake = self.wake()
        with mock.patch.object(D, "delete_native_queue_submission", side_effect=ValueError("temporarily unavailable")):
            self.assertFalse(wake.acknowledge_remote(self.ids[0]))
        failed = self.receipt(self.ids[0])
        with mock.patch.object(D.time, "time", return_value=failed["withdrawal_attempt_at"] + 31), mock.patch.object(D, "delete_native_queue_submission", return_value=True) as delete, mock.patch.object(D, "atomic_json", wraps=D.atomic_json) as write:
            self.assertFalse(wake.acknowledge_remote(self.ids[0], wait=False))
            wake.close(wait=True)
        delete.assert_called_once()
        self.assertEqual(self.receipt(self.ids[0])["queue_disposition"], "removed")
        self.assertFalse(any("acknowledgments" in str(call.args[0]) for call in write.call_args_list))



if __name__ == "__main__":
    unittest.main()
