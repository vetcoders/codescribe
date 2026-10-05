"""Written messages use immutable recipients, canonical publication and native wakeup."""
import argparse
from contextlib import closing
import importlib.util
import io
import json
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("bus_user_text", Path(__file__).resolve().parents[1] / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

class UserTextTests(unittest.TestCase):
    def test_five_equal_messages_keep_distinct_delivery_ids_without_pcm(self):
        with tempfile.TemporaryDirectory() as directory:
            with closing(DEMUX.SessionLease(root=Path(directory), provider="codex", provider_session_id="agent-a",
                    name="lena", bus=Path(directory) / "bus", requested_id=None, ttl_seconds=30,
                    follow_from_end=False, coalesce=True)) as lease:
                ids = []
                for index in range(5):
                    identity = f"{index + 1:024x}"
                    event = {"schema": DEMUX.AGENT_USER_MESSAGE_SCHEMA, "kind": "agent_user_message",
                             "source": "typed", "message_id": identity, "source_event_id": identity,
                             "audience": "lena", "text": "Iwo"}
                    events = DEMUX.normalized_revision_events(json.dumps(event), DEMUX.EvidenceNormalizer())
                    self.assertEqual(len(events), 1)
                    payload = DEMUX.consider(events[0], name="lena", hear_all=False, drafts=False, debug=False)
                    self.assertEqual(payload["kind"], "message")
                    self.assertNotIn("status", payload)
                    self.assertNotIn("wav", payload)
                    lease.enrich(payload)
                    self.assertTrue(lease.queue_delivery(payload))
                    self.assertFalse(lease.queue_delivery(payload))
                    ids.append(payload["delivery_id"])
                self.assertEqual(len(set(ids)), 5)
                self.assertEqual(len(lease.pending), 5)

    def test_publish_holds_binding_identity_and_refuses_rebound_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            bus = root / "bus.jsonl"
            lease_id = DEMUX.lease_identifier("codex", "agent-a")
            DEMUX.write_channel_binding(root, "2", "lena", "codex", "agent-a", str(bus))
            DEMUX.atomic_json(root / "leases" / f"{lease_id}.json", {
                "schema": DEMUX.LEASE_SCHEMA, "provider": "codex", "provider_session_id": "agent-a",
                "lease_id": lease_id, "bus": str(bus)})
            args = argparse.Namespace(bridge_home=root, channel="2", bus=bus, lease=lease_id,
                                      provider="codex", session="agent-a")
            with patch.object(DEMUX, "live_follower_pid", return_value=123), \
                 patch.object(DEMUX, "publish_reply_event", return_value={"stream_id":"fixture", "stream_dev":1, "stream_inode":2, "offset":0, "length":12}) as publish, \
                 patch.object(DEMUX, "emit"), patch.object(sys, "stdin", io.TextIOWrapper(io.BytesIO("Iwo".encode()))):
                self.assertEqual(DEMUX.send_text_command(args), 0)
                event = publish.call_args.args[1]
                self.assertEqual(event["recipients"][0]["lease_id"], lease_id)
                self.assertEqual(event["source"], "typed")
                self.assertNotIn("sample_start", event)
                self.assertNotIn("status", event)
                state = DEMUX.read_json(root / DEMUX.AUDIENCE_BINDING_FILENAME)
                state["bindings"]["2"]["provider_session_id"] = "foreign"
                DEMUX.atomic_json(root / DEMUX.AUDIENCE_BINDING_FILENAME, state)
                with self.assertRaises(ValueError): DEMUX.send_text_command(args)
                self.assertEqual(publish.call_count, 1)

class ChannelCaptureMessageTests(unittest.TestCase):
    """One capture message retains every exact physical observation."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.lease = DEMUX.SessionLease(root=self.root, provider="codex", provider_session_id="agent-a",
            name="lena", bus=self.bus, requested_id=None, ttl_seconds=30, follow_from_end=False)
        self.addCleanup(self.lease.close)
        self.normalizer = DEMUX.EvidenceNormalizer()

    def evidence(self, start=0, revision=1, session="agent-channel-2-five", text="Iwo Iwo Iwo Iwo Iwo",
                 action="record_ledger_terminal_seal", document_index=0):
        return {"schema": DEMUX.EVIDENCE_SCHEMA, "session_id": session,
            "occurrence_session_id": session, "capture_epoch": 1,
            "sample_start": start, "sample_end": start + 1600, "document_index": document_index,
            "reducer_revision": revision, "sequence": revision, "reducer_action": action,
            "rendered_text": text, "label": "Iwo", "audience": "lena",
            "emitted_at": "2026-10-05T10:00:00Z",
            "acoustic_receipts": [{"receipt_id": f"pcm-{start}"}]}

    def close_capture(self, session="agent-channel-2-five"):
        return DEMUX.normalized_revision_events(json.dumps({"schema": DEMUX.CLEAN_SCHEMA,
            "session_id": session, "status": DEMUX.SESSION_ENDED}), self.normalizer)

    def envelopes(self, rows):
        values = []
        for row in rows:
            payload = DEMUX.consider(row, name="lena", hear_all=False, drafts=True, debug=False)
            if payload:
                self.lease.enrich(payload)
                values.append(payload)
        return values

    @staticmethod
    def ranges(row):
        return [(item["occurrence_session_id"], item["capture_epoch"],
                 item["sample_start"], item["sample_end"]) for item in row["occurrences"]]

    def test_shared_revision_delivers_once_with_five_complete_pcm_receipts(self):
        row = self.evidence()
        row["persistence_encoding"] = "shared-revision.v1"
        fields = ("sequence", "capture_epoch", "sample_start", "sample_end", "document_index",
                  "emitted_at", "occurrence_session_id", "label", "acoustic_receipts")
        row["occurrence_rows"] = [
            {key: self.evidence(start=index * 3200)[key] for key in fields}
            for index in range(1, 5)]
        previews = DEMUX.normalized_revision_events(json.dumps(row), self.normalizer)
        self.assertEqual(len(previews), 1)
        self.assertNotEqual(previews[0]["status"], DEMUX.SEALED)
        final = [event for event in self.close_capture() if event.get("status") == DEMUX.SEALED]
        self.assertEqual(len(final), 1)
        payload = self.envelopes(final)[0]
        self.assertEqual(payload["text"], row["rendered_text"])
        self.assertEqual(self.ranges(payload), [(row["session_id"], 1, index * 3200, index * 3200 + 1600) for index in range(5)])
        self.assertEqual([item["label"] for item in payload["occurrences"]], ["Iwo"] * 5)
        self.assertEqual([item["acoustic_receipts"] for item in payload["occurrences"]],
                         [[{"receipt_id": f"pcm-{index * 3200}"}] for index in range(5)])
        self.assertTrue(self.lease.queue_delivery(payload))
        self.assertFalse(self.lease.queue_delivery(payload))
        self.assertEqual(len(self.lease.pending), 1)
        self.assertEqual(len(self.ranges(DEMUX.receipt_envelope(payload, str(self.bus)))), 5)
        self.assertFalse(any(event.get("status") == DEMUX.SEALED for event in self.close_capture()))

    def test_five_equal_captures_have_five_distinct_message_and_delivery_ids(self):
        messages, deliveries = [], []
        for index in range(5):
            session = f"agent-channel-2-capture-{index}"
            self.normalizer.normalize(self.evidence(session=session))
            final = [row for row in self.close_capture(session) if row.get("status") == DEMUX.SEALED]
            payload = self.envelopes(final)[0]
            messages.append(payload["message_id"])
            deliveries.append(payload["delivery_id"])
            self.assertTrue(self.lease.queue_delivery(payload))
        self.assertEqual(len(set(messages)), 5)
        self.assertEqual(len(set(deliveries)), 5)
        self.assertEqual(len(self.lease.pending), 5)

    def test_stale_first_observed_range_survives_without_replacing_new_text_or_receipt(self):
        latest = self.evidence(revision=10, text="Nowy pełny tekst")
        self.normalizer.normalize(latest)
        stale = self.evidence(start=3200, revision=1, text="Stary tekst")
        self.assertIsNone(self.normalizer.normalize(stale))
        stale_same = self.evidence(revision=1, text="Stary tekst")
        stale_same["acoustic_receipts"] = [{"receipt_id": "stale"}]
        self.assertIsNone(self.normalizer.normalize(stale_same))
        final = [row for row in self.close_capture() if row.get("status") == DEMUX.SEALED][0]
        self.assertEqual(final["text"], latest["rendered_text"])
        self.assertEqual(len(set(self.ranges(final))), 2)
        self.assertEqual(final["occurrences"][0]["acoustic_receipts"], latest["acoustic_receipts"])

    def test_document_position_changes_neither_duplicate_nor_drop_physical_identity(self):
        for index in range(5):
            self.normalizer.normalize(self.evidence(start=index * 3200, revision=index + 1))
        self.normalizer.normalize(self.evidence(revision=10, document_index=4))
        final = [row for row in self.close_capture() if row.get("status") == DEMUX.SEALED][0]
        self.assertEqual(len(self.ranges(final)), 5)
        self.assertEqual(len(set(self.ranges(final))), 5)

    def test_quiet_restart_preserves_unclosed_inventory_and_final_delivery_identity(self):
        for index in range(5):
            self.normalizer.normalize(self.evidence(start=index * 3200, revision=index + 1))
        saved = json.loads(json.dumps(list(self.normalizer.channel_documents().values())))
        original = self.envelopes([row for row in self.close_capture() if row.get("status") == DEMUX.SEALED])[0]
        self.normalizer = DEMUX.EvidenceNormalizer()
        self.normalizer.restore_channel_documents(saved)
        recovered = self.envelopes([row for row in self.close_capture() if row.get("status") == DEMUX.SEALED])[0]
        self.assertEqual(recovered["delivery_id"], original["delivery_id"])
        self.assertEqual(recovered["message_id"], original["message_id"])
        self.assertEqual(recovered["text"], original["text"])
        self.assertEqual(recovered["occurrences"], original["occurrences"])

    def test_real_follower_ack_and_reopen_keep_unclosed_capture_until_final_delivery(self):
        self.lease.close()
        rows = []
        for index in range(5):
            row = self.evidence(start=index * 3200, revision=index + 1)
            row["recipients"] = [{"provider": "codex", "provider_session_id": "agent-a",
                "lease_id": self.lease.lease_id, "bus": self.lease.bus, "channel": "2", "name": "lena"}]
            row["acoustic_receipts"] = [{"receipt_id": f"pcm-{index}", "text": "private acoustic words"}]
            rows.append(row)
        self.bus.write_text("".join(json.dumps(row) + "\n" for row in rows))
        base = [sys.executable, str(SPEC.origin), "--bus", str(self.bus), "--bridge-home", str(self.root),
                "--provider", "codex", "--session", "agent-a"]
        def follow():
            result = subprocess.run(base + ["--name", "lena", "--from-start", "--drafts", "--coalesce"],
                                    capture_output=True, text=True, timeout=10, check=True)
            return [json.loads(line) for line in result.stdout.splitlines()]
        first = follow()
        state = DEMUX.read_json(self.lease.path)
        self.assertEqual(state["cursor"], self.bus.stat().st_size)
        self.assertEqual(len(state["pending"]), 1)
        preview = state["pending"][0]
        self.assertNotEqual(preview["kind"], "seal")
        self.assertEqual(len(self.ranges(preview)), 5)
        self.assertTrue(first)
        subprocess.run(base + ["--ack", preview["delivery_id"]], capture_output=True, timeout=10, check=True)
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease.lease_id / (preview["delivery_id"] + ".json"))
        encoded = json.dumps(marker)
        self.assertNotIn("private acoustic words", encoded)
        self.assertNotIn(preview["text"], encoded)
        self.assertNotIn('"label"', encoded)
        self.assertNotIn('"acoustic_receipts"', encoded)
        self.assertEqual(self.ranges(marker["envelope"]), self.ranges(preview))
        quiet = follow()
        self.assertFalse(any(row.get("kind") in ("draft", "revised", "seal") for row in quiet))
        state = DEMUX.read_json(self.lease.path)
        self.assertEqual(state["pending"], [])
        self.assertEqual(len(self.ranges(state["unclosed_channel_messages"][rows[0]["session_id"]])), 5)
        with self.bus.open("a") as stream:
            stream.write(json.dumps({"schema": DEMUX.CLEAN_SCHEMA, "session_id": rows[0]["session_id"],
                                     "status": DEMUX.SESSION_ENDED}) + "\n")
        final = [row for row in follow() if row.get("kind") == "seal"]
        self.assertEqual(len(final), 1)
        self.assertEqual(final[0]["text"], preview["text"])
        self.assertEqual(self.ranges(final[0]), self.ranges(preview))
        self.assertNotEqual(final[0]["delivery_id"], preview["delivery_id"])
        state = DEMUX.read_json(self.lease.path)
        self.assertEqual(state["unclosed_channel_messages"], {})
        self.assertEqual(state["cursor"], self.bus.stat().st_size)
        replay = [row for row in follow() if row.get("kind") == "seal"]
        self.assertEqual([row["delivery_id"] for row in replay], [final[0]["delivery_id"]],
                         "unread final delivery survives without acquiring a new identity")
        subprocess.run(base + ["--ack", final[0]["delivery_id"]], capture_output=True, timeout=10, check=True)
        self.assertFalse(any(row.get("kind") == "seal" for row in follow()), "acknowledged final is not replayed")

    def test_ack_preview_after_queue_buffer_error_preserves_uncommitted_capture_on_restart(self):
        self.lease.close()
        row = self.evidence()
        row["recipients"] = [{"provider": "codex", "provider_session_id": "agent-a",
            "lease_id": self.lease.lease_id, "bus": self.lease.bus, "channel": "2", "name": "lena"}]
        row["persistence_encoding"] = "shared-revision.v1"
        fields = ("sequence", "capture_epoch", "sample_start", "sample_end", "document_index",
                  "emitted_at", "occurrence_session_id", "label", "acoustic_receipts")
        row["occurrence_rows"] = [
            {key: self.evidence(start=index * 3200)[key] for key in fields}
            for index in range(1, 5)]
        self.bus.write_text(json.dumps(row) + "\n")
        base = [str(SPEC.origin), "--bus", str(self.bus), "--bridge-home", str(self.root),
                "--provider", "codex", "--session", "agent-a"]

        def invoke(*arguments):
            output = io.StringIO()
            with patch.object(sys, "argv", base + list(arguments)), \
                 patch.object(sys, "stdout", output), patch.object(sys, "stderr", io.StringIO()):
                code = DEMUX.main()
            return code, [json.loads(line) for line in output.getvalue().splitlines()]

        original_emit = DEMUX.emit_follower

        def refuse_publication(payload, *arguments, **options):
            if payload.get("kind") in ("draft", "revised"):
                persisted = DEMUX.read_json(self.lease.path)
                self.assertIn(payload["delivery_id"],
                              [item["delivery_id"] for item in persisted["pending"]])
                self.assertEqual(persisted["cursor"], 0)
                raise BufferError("injected after durable queue, before cursor commit")
            return original_emit(payload, *arguments, **options)

        with patch.object(DEMUX, "emit_follower", side_effect=refuse_publication):
            code, _ = invoke("--name", "lena", "--from-start", "--drafts", "--coalesce")
        self.assertEqual(code, 4)
        interrupted = DEMUX.read_json(self.lease.path)
        self.assertEqual(interrupted["cursor"], 0)
        self.assertEqual(len(interrupted["pending"]), 1)
        preview = interrupted["pending"][0]
        self.assertEqual(len(set(self.ranges(preview))), 5)
        self.assertEqual(invoke("--ack", preview["delivery_id"])[0], 0)

        code, recovered = invoke("--name", "lena", "--from-start", "--drafts", "--coalesce")
        self.assertEqual(code, 0)
        self.assertFalse(any(item.get("kind") in ("draft", "revised", "seal") for item in recovered))
        restored = DEMUX.read_json(self.lease.path)
        self.assertEqual(restored["cursor"], self.bus.stat().st_size)
        self.assertEqual(restored["pending"], [])
        self.assertIn(row["session_id"], restored["unclosed_channel_messages"],
                      "ACK must not erase the observed capture waiting for manual close")
        self.assertEqual(self.ranges(restored["unclosed_channel_messages"][row["session_id"]]),
                         self.ranges(preview))
        with self.bus.open("a") as stream:
            stream.write(json.dumps({"schema": DEMUX.CLEAN_SCHEMA, "session_id": row["session_id"],
                                     "status": DEMUX.SESSION_ENDED}) + "\n")
        code, terminal = invoke("--name", "lena", "--from-start", "--drafts", "--coalesce")
        self.assertEqual(code, 0)
        final = [item for item in terminal if item.get("kind") == "seal"]
        self.assertEqual(len(final), 1)
        self.assertEqual(final[0]["message_id"], preview["message_id"])
        self.assertNotEqual(final[0]["delivery_id"], preview["delivery_id"])
        self.assertEqual(final[0]["text"], preview["text"])
        self.assertEqual(final[0]["occurrences"], preview["occurrences"])
        self.assertEqual(DEMUX.read_json(self.lease.path)["unclosed_channel_messages"], {})
        self.assertEqual(invoke("--ack", final[0]["delivery_id"])[0], 0)
        self.assertFalse(any(item.get("kind") == "seal" for item in
                             invoke("--name", "lena", "--from-start", "--drafts", "--coalesce")[1]))

    def test_terminal_queue_buffer_error_preserves_final_inventory_and_cursor_on_restart(self):
        self.lease.close()
        row = self.evidence()
        row["recipients"] = [{"provider": "codex", "provider_session_id": "agent-a",
            "lease_id": self.lease.lease_id, "bus": self.lease.bus, "channel": "2", "name": "lena"}]
        row["persistence_encoding"] = "shared-revision.v1"
        fields = ("sequence", "capture_epoch", "sample_start", "sample_end", "document_index",
                  "emitted_at", "occurrence_session_id", "label", "acoustic_receipts")
        row["occurrence_rows"] = [
            {key: self.evidence(start=index * 3200)[key] for key in fields}
            for index in range(1, 5)]
        evidence_line = json.dumps(row) + "\n"
        close = {"schema": DEMUX.CLEAN_SCHEMA, "session_id": row["session_id"],
                 "status": DEMUX.SESSION_ENDED}
        self.bus.write_text(evidence_line + json.dumps(close) + "\n")
        expected_ranges = [(row["session_id"], 1, index * 3200, index * 3200 + 1600)
                           for index in range(5)]
        base = [str(SPEC.origin), "--bus", str(self.bus), "--bridge-home", str(self.root),
                "--provider", "codex", "--session", "agent-a"]

        def invoke(*arguments):
            output = io.StringIO()
            with patch.object(sys, "argv", base + list(arguments)), \
                 patch.object(sys, "stdout", output), patch.object(sys, "stderr", io.StringIO()):
                code = DEMUX.main()
            return code, [json.loads(line) for line in output.getvalue().splitlines()]

        original_emit = DEMUX.emit_follower
        interrupted_payloads = []

        def refuse_terminal(payload, *arguments, **options):
            if payload.get("kind") == "seal":
                persisted = DEMUX.read_json(self.lease.path)
                self.assertEqual(persisted["cursor"], len(evidence_line.encode()))
                self.assertEqual(persisted["pending"], [payload])
                interrupted_payloads.append(payload)
                raise BufferError("injected after durable final, before close cursor commit")
            return original_emit(payload, *arguments, **options)

        with patch.object(DEMUX, "emit_follower", side_effect=refuse_terminal):
            self.assertEqual(invoke("--name", "lena", "--from-start", "--coalesce")[0], 4)
        self.assertEqual(len(interrupted_payloads), 1)
        original = interrupted_payloads[0]
        self.assertEqual(self.ranges(original), expected_ranges)
        self.assertEqual(original["text"], row["rendered_text"])
        self.assertEqual([item["label"] for item in original["occurrences"]], ["Iwo"] * 5)
        self.assertEqual(len({item["acoustic_receipts"][0]["receipt_id"]
                              for item in original["occurrences"]}), 5)

        code, recovered = invoke("--name", "lena", "--from-start", "--coalesce")
        self.assertEqual(code, 0)
        finals = [item for item in recovered if item.get("kind") == "seal"]
        self.assertEqual(finals, [original], "restart must emit the same complete final exactly once")
        state = DEMUX.read_json(self.lease.path)
        self.assertEqual(state["cursor"], self.bus.stat().st_size)
        self.assertEqual(state["pending"], [original])
        self.assertEqual(state["unclosed_channel_messages"], {})
        self.assertEqual(invoke("--ack", original["delivery_id"])[0], 0)
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease.lease_id /
                                (original["delivery_id"] + ".json"))
        encoded = json.dumps(marker)
        self.assertNotIn('"label"', encoded)
        self.assertNotIn('"acoustic_receipts"', encoded)
        self.assertNotIn(original["text"], encoded)
        self.assertEqual(self.ranges(marker["envelope"]), expected_ranges)
        self.assertFalse(any(item.get("kind") == "seal" for item in
                             invoke("--name", "lena", "--from-start", "--coalesce")[1]))

    def test_late_close_only_settles_its_named_predecessor(self):
        self.normalizer.normalize(self.evidence(session="agent-channel-2-old"))
        self.normalizer.normalize(self.evidence(session="agent-channel-2-new", text="Nowa wiadomość"))
        old = [row for row in self.close_capture("agent-channel-2-old") if row.get("status") == DEMUX.SEALED]
        self.assertEqual([row["session_id"] for row in old], ["agent-channel-2-old"])
        self.assertIn("agent-channel-2-new", self.normalizer.channel_documents())
        new = [row for row in self.close_capture("agent-channel-2-new") if row.get("status") == DEMUX.SEALED]
        self.assertEqual(new[0]["text"], "Nowa wiadomość")

    def test_uncertified_capture_retains_refusal_diagnostic_and_all_ranges(self):
        for index in range(5):
            self.normalizer.normalize(self.evidence(start=index * 3200, revision=index + 1, action="seal_coverage"))
        final = self.envelopes([row for row in self.close_capture() if row.get("status") == DEMUX.SEALED])[0]
        self.assertEqual(final["coverage"], "refused")
        self.assertFalse(final["state_change_allowed"])
        self.assertEqual(len(set(self.ranges(final))), 5)

if __name__ == "__main__": unittest.main()
