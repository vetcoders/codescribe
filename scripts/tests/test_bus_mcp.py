"""Native MCP reads owned bus deliveries; only an explicit ACK consumes them."""
import hashlib
import importlib.util
import json
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
DEMUX_SPEC = importlib.util.spec_from_file_location("bus_mcp_demux_tests", SCRIPTS / "bus-demux.py")
DEMUX = importlib.util.module_from_spec(DEMUX_SPEC)
sys.modules[DEMUX_SPEC.name] = DEMUX
DEMUX_SPEC.loader.exec_module(DEMUX)
MCP_SPEC = importlib.util.spec_from_file_location("bus_mcp_tests", SCRIPTS / "bus-mcp.py")
MCP = importlib.util.module_from_spec(MCP_SPEC)
sys.modules[MCP_SPEC.name] = MCP
MCP_SPEC.loader.exec_module(MCP)


class MailboxFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix=".bus-mcp-test-", dir=Path(__file__).parent)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.bus = self.root / "bus.jsonl"
        self.bus.touch()
        self.session = "11111111-2222-4333-8444-555555555555"
        self.lease = DEMUX.lease_identifier("junie", self.session)
        self.lease_path = self.root / "leases" / f"{self.lease}.json"
        self.ids = [f"{n:024x}" for n in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        self.save()
        self.mailbox = MCP.Mailbox(DEMUX, self.root, self.bus)
        self.mailbox.owner = self.session

    def envelope(self, identity):
        owner = {"lease_id": self.lease, "provider": "junie", "provider_session_id": self.session}
        return dict(owner, delivery_owner=dict(owner), delivery_id=identity,
                    kind="message", source="typed", text="Iwo", bus=str(self.bus),
                    emitted_at="2026-10-11T19:03:00Z", message_id=identity)

    def save(self):
        DEMUX.atomic_json(self.lease_path, {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease, "provider": "junie",
            "provider_session_id": self.session, "bus": str(self.bus),
            "pending": self.pending, "cursor": 0,
        })

    def call(self, name, values=None, cancel=None):
        return self.mailbox.call(f"codescribe_{name}", {} if values is None else values,
                                 threading.Event() if cancel is None else cancel)

    def assert_not_acknowledged(self):
        self.assertEqual(self.mailbox.acked_ids, set())
        self.assertFalse((self.root / "acknowledgments").exists())

    def start_wait(self):
        cancel = threading.Event()
        outcome = queue.Queue()

        def wait():
            try:
                outcome.put(("result", self.call("wait", cancel=cancel)))
            except Exception as error:
                outcome.put(("error", error))

        thread = threading.Thread(target=wait, daemon=True)
        thread.start()

        def stop():
            cancel.set()
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive(), "mailbox waiter did not stop after cancellation")

        self.addCleanup(stop)
        deadline = time.monotonic() + 3
        while not self.mailbox.wait_lock.locked() and thread.is_alive() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(self.mailbox.wait_lock.locked(), "waiter never became active")
        return cancel, thread, outcome


class MailboxTests(MailboxFixture):
    def test_read_returns_complete_owned_envelope_without_ack(self):
        self.pending[0]["text"] = "Iwo — cała wiadomość, nie podgląd. " * 100
        self.pending[0]["attachments"] = [{"path": str(self.root / "draft.txt"), "name": "draft.txt",
                                            "media_type": "text/plain", "bytes": 4, "sha256": "a" * 64}]
        self.save()
        before = self.lease_path.read_bytes()
        result = self.call("read")
        self.assertEqual(result["deliveries"], self.pending[:1])
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(result["remaining"], 4)
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.call("read"), result)
        self.assertEqual(self.lease_path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_wait_returns_existing_message_without_ack(self):
        before = self.lease_path.read_bytes()
        result = self.call("wait")
        self.assertEqual(result["deliveries"], self.pending[:1])
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.lease_path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_five_identical_iwo_remain_five_distinct_reads_and_acks(self):
        read_ids = []
        ack_ids = []
        for index, identity in enumerate(self.ids):
            result = self.call("read")
            self.assertEqual(result["deliveries"], [self.pending[index]])
            self.assertEqual(result["read_delivery_ids"], [identity])
            self.assertEqual(result["remaining"], 4 - index)
            read_ids.extend(result["read_delivery_ids"])
            self.assertFalse((self.root / "acknowledgments" / self.lease / f"{identity}.json").exists())
            receipt = self.call("ack", {"delivery_ids": result["read_delivery_ids"]})
            self.assertEqual(receipt["kind"], "acknowledged")
            self.assertTrue(receipt["native_queue_settled"])
            ack_ids.append(receipt["delivery_id"])
            marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{identity}.json")
            self.assertEqual(marker["delivery_id"], identity)
            self.assertEqual(marker["provider_session_id"], self.session)
        self.assertEqual(read_ids, self.ids)
        self.assertEqual(ack_ids, self.ids)
        self.assertEqual(self.mailbox.read_ids, set(self.ids))
        self.assertEqual(self.mailbox.acked_ids, set(self.ids))
        self.assertEqual(len(list((self.root / "acknowledgments" / self.lease).iterdir())), 5)
        self.assertEqual(self.call("read")["deliveries"], [])

    def test_ack_requires_read_and_refuses_whole_mixed_batch(self):
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids})
        self.call("read")
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:2]})
        self.assert_not_acknowledged()

    def test_ack_is_idempotent_without_rewriting_read_receipt(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        marker = self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json"
        before = marker.read_bytes()
        self.call("ack", {"delivery_ids": self.ids[:1]})
        self.assertEqual(marker.read_bytes(), before)
        self.assertEqual(self.mailbox.acked_ids, {self.ids[0]})

    def test_read_refuses_foreign_owner_even_beyond_first_envelope(self):
        for field in ("provider", "provider_session_id", "lease_id"):
            with self.subTest(field=field):
                self.pending = [self.envelope(identity) for identity in self.ids]
                self.pending[-1]["delivery_owner"][field] = "foreign"
                self.save()
                with self.assertRaisesRegex(ValueError, "foreign pending delivery owner"):
                    self.call("read")
                self.assertEqual(self.mailbox.read_ids, set())
                self.assert_not_acknowledged()

    def test_wait_refuses_foreign_envelope_without_marking_it_read(self):
        self.pending[0]["provider_session_id"] = "foreign"
        self.save()
        with self.assertRaisesRegex(ValueError, "foreign"):
            self.call("wait")
        self.assertFalse(self.mailbox.wait_lock.locked())
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_ack_rechecks_lease_owner_after_read(self):
        self.call("read")
        state = DEMUX.read_json(self.lease_path)
        state["provider_session_id"] = "foreign"
        DEMUX.atomic_json(self.lease_path, state)
        with self.assertRaisesRegex(ValueError, "does not belong"):
            self.call("ack", {"delivery_ids": self.ids[:1]})
        self.assert_not_acknowledged()

    def test_oversized_read_and_wait_refuse_without_read_or_ack(self):
        self.pending[0]["text"] = "Iwo " * 300000
        self.save()
        for operation in ("read", "wait"):
            with self.subTest(operation=operation), self.assertRaisesRegex(ValueError, "read-bytes"):
                self.call(operation)
            self.assertEqual(self.mailbox.read_ids, set())
            self.assert_not_acknowledged()
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:1]})

    def test_wait_remains_pending_until_future_arrival_without_ack(self):
        self.pending = []
        self.save()
        _cancel, thread, outcome = self.start_wait()
        with self.assertRaises(queue.Empty):
            outcome.get(timeout=0.2)
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        kind, result = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(kind, "result", result)
        self.assertEqual(result["deliveries"], self.pending)
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assert_not_acknowledged()

    def test_cancel_wait_releases_waiter_without_read_or_ack(self):
        self.pending = []
        self.save()
        cancel, thread, outcome = self.start_wait()
        cancel.set()
        kind, error = outcome.get(timeout=5)
        thread.join(timeout=5)
        self.assertEqual(kind, "error")
        self.assertIsInstance(error, ValueError)
        self.assertIn("cancel", str(error))
        self.assertFalse(thread.is_alive())
        self.assertFalse(self.mailbox.wait_lock.locked())
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()
        self.pending = [self.envelope(self.ids[0])]
        self.save()
        self.assertEqual(self.call("wait")["read_delivery_ids"], self.ids[:1])

    def test_already_cancelled_wait_does_not_read_pending_message(self):
        cancel = threading.Event()
        cancel.set()
        with self.assertRaisesRegex(ValueError, "cancel"):
            self.call("wait", cancel=cancel)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_only_one_waiter_is_active(self):
        self.pending = []
        self.save()
        cancel, thread, outcome = self.start_wait()
        with self.assertRaisesRegex(ValueError, "already active"):
            self.call("wait")
        self.assertTrue(thread.is_alive())
        cancel.set()
        self.assertEqual(outcome.get(timeout=5)[0], "error")
        self.assert_not_acknowledged()

    def test_connect_cannot_change_owner_or_receipt_sets(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        with self.assertRaisesRegex(ValueError, "another session"):
            self.call("connect", {"session_id": "other-native-session", "name": "Roman", "channel": "3"})
        self.assertEqual(self.mailbox.owner, self.session)
        self.assertEqual(self.mailbox.read_ids, {self.ids[0]})
        self.assertEqual(self.mailbox.acked_ids, {self.ids[0]})

    def test_connect_validates_identity_name_and_channel_before_side_effects(self):
        self.mailbox.owner = None
        valid = {"session_id": self.session, "name": "Roman", "channel": "3"}
        for field, value in (("session_id", ""), ("session_id", "\n"), ("session_id", 3),
                             ("name", " "), ("name", None), ("channel", "0"),
                             ("channel", "10"), ("channel", 3)):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                self.call("connect", dict(valid, **{field: value}))
            self.assertIsNone(self.mailbox.owner)
        self.assertFalse((self.root / DEMUX.AUDIENCE_BINDING_FILENAME).exists())

    def test_failed_connect_keeps_existing_slot_and_restores_unconnected_owner(self):
        self.mailbox.owner = None
        foreign = {"audience": "someone-else", "provider": "junie",
                   "provider_session_id": "foreign", "bus": str(self.bus)}
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings["3"] = foreign
        path = self.root / DEMUX.AUDIENCE_BINDING_FILENAME
        before = path.read_bytes()
        with self.assertRaises(ValueError):
            self.call("connect", {"session_id": self.session, "name": "Roman", "channel": "3"})
        self.assertIsNone(self.mailbox.owner)
        self.assertEqual(path.read_bytes(), before)
        self.assert_not_acknowledged()

    def test_all_session_tools_refuse_before_connect(self):
        self.mailbox.owner = None
        operations = [("read", {}), ("wait", {}), ("status", {}), ("detach", {}),
                      ("ack", {"delivery_ids": self.ids[:1]}),
                      ("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})]
        for name, values in operations:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "connect"):
                self.call(name, values)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_invalid_tool_arguments_do_not_change_receipts(self):
        operations = [("read", {"session_id": "foreign"}), ("wait", {"timeout": 1}),
                      ("ack", {}), ("ack", {"delivery_ids": []}),
                      ("ack", {"delivery_ids": self.ids[0]}), ("reply", {"text": "Odpowiedź"}),
                      ("unknown", {}), ("status", [])]
        for name, values in operations:
            with self.subTest(name=name, values=values), self.assertRaises(ValueError):
                self.call(name, values)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assertEqual(self.mailbox.replied_ids, set())
        self.assert_not_acknowledged()

    def test_no_bus_override_reads_lease_bus_without_touching_default_bus(self):
        self.mailbox = MCP.Mailbox(DEMUX, self.root, None)
        self.mailbox.owner = self.session
        self.assertEqual(self.call("read")["deliveries"], self.pending[:1])
        self.call("ack", {"delivery_ids": self.ids[:1]})
        marker = DEMUX.read_json(self.root / "acknowledgments" / self.lease / f"{self.ids[0]}.json")
        self.assertEqual(marker["bus"], str(self.bus))

    def test_status_observes_backlog_without_read_or_ack(self):
        result = self.call("status")
        self.assertEqual(result["kind"], "status")
        self.assertEqual(result["lease_id"], self.lease)
        self.assertEqual(result["backlog"], 5)
        self.assertEqual(self.mailbox.read_ids, set())
        self.assert_not_acknowledged()

    def test_detach_preserves_unread_deliveries_without_ack(self):
        result = self.call("detach")
        self.assertEqual(result["kind"], "detach_receipt")
        self.assertEqual(result["unacked_deliveries"], 5)
        self.assertEqual(result["unacked_delivery_ids"], self.ids)
        self.assertIsNone(self.mailbox.owner)
        self.assertEqual(DEMUX.read_json(self.lease_path)["pending"], self.pending)
        self.assert_not_acknowledged()

    def test_detach_drained_mailbox_requires_connect_again(self):
        self.pending = []
        self.save()
        result = self.call("detach")
        self.assertEqual(result["kind"], "detach_receipt")
        self.assertEqual(result["unacked_deliveries"], 0)
        self.assertIsNone(self.mailbox.owner)
        with self.assertRaisesRegex(ValueError, "connect"):
            self.call("read")

    def test_reply_requires_ack_not_just_read(self):
        values = {"delivery_id": self.ids[0], "text": "Odpowiedź"}
        with self.assertRaisesRegex(ValueError, "acknowledged"):
            self.call("reply", values)
        self.call("read")
        with self.assertRaisesRegex(ValueError, "acknowledged"):
            self.call("reply", values)
        self.assertEqual(self.mailbox.replied_ids, set())
        self.assert_not_acknowledged()

    def test_reply_speaks_once_using_exact_acked_id_and_owned_root(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        text = "Gotowe — pełna odpowiedź."
        receipt = {"kind": "voice_reply", "delivery_id": self.ids[0], "spoken": True}
        with patch.object(MCP.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 0, json.dumps(receipt), "")) as tts:
            self.assertEqual(self.call("reply", {"delivery_id": self.ids[0], "text": text}), receipt)
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": text})
        tts.assert_called_once()
        command = tts.call_args.args[0]
        self.assertEqual(command[:2], [sys.executable, str(SCRIPTS / "cs-bus")])
        self.assertEqual(command[command.index("--bridge-home") + 1], str(self.root))
        self.assertEqual(command[command.index("--bus") + 1], str(self.bus))
        self.assertEqual(command[command.index("--provider") + 1], "junie")
        self.assertEqual(command[command.index("--session") + 1], self.session)
        self.assertEqual(command[-4:], ["--say", text, "--reply-to", self.ids[0]])
        self.assertEqual(self.mailbox.replied_ids, {self.ids[0]})

    def test_failed_voice_reply_keeps_receipt_and_cannot_be_replayed(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        receipt = {"kind": "voice_reply", "spoken": False, "reason": "recording_active"}
        with patch.object(MCP.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 3, json.dumps(receipt), "playback refused")) as tts:
            result = self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
            self.assertEqual(result, {"receipts": [receipt], "exit_code": 3})
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
        tts.assert_called_once()

    def test_tts_timeout_is_an_attempt_not_permission_to_reply_again(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        with patch.object(MCP.subprocess, "run", side_effect=subprocess.TimeoutExpired("tts", 300)) as tts:
            with self.assertRaises(subprocess.TimeoutExpired):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
            with self.assertRaisesRegex(ValueError, "already attempted"):
                self.call("reply", {"delivery_id": self.ids[0], "text": "Odpowiedź"})
        tts.assert_called_once()
        self.assertEqual(self.mailbox.replied_ids, {self.ids[0]})

    def test_invalid_reply_text_does_not_consume_attempt(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        for text in ("", " \n", None, 1):
            error = "text" if isinstance(text, str) else "type"
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, error):
                self.call("reply", {"delivery_id": self.ids[0], "text": text})
        self.assertEqual(self.mailbox.replied_ids, set())


class ExtensionInstallTests(MailboxFixture):
    def setUp(self):
        super().setUp()
        self.source = self.root / "runtime" / "plugins"
        marketplace = self.source / ".claude-plugin" / "marketplace.json"
        DEMUX.atomic_json(marketplace, {"name": "codescribe", "plugins": [{"name": "codescribe", "source": "./codescribe"}]})
        self.directory = self.root / "extensions"
        self.target = self.directory / "extensions.json"
        self.identity = "native-registered-marketplace"

    def install(self):
        return MCP.install_extension(DEMUX, self.root, self.directory)

    def test_install_preserves_foreign_entries_and_is_idempotent(self):
        foreign = {"remote-other": {"source": {"type": "git", "url": "https://example.invalid/plugins"},
                                    "extensions": ["other"], "custom": {"keep": True}}}
        registered = {"source": {"type": "local", "path": str(self.source)}, "extensions": []}
        DEMUX.atomic_json(self.target, {**foreign, self.identity: registered})
        receipt = self.install()
        config = DEMUX.read_json(self.target)
        self.assertEqual(receipt["kind"], "extension_registered")
        self.assertEqual(receipt["source"], str(self.source))
        self.assertEqual(receipt["configuration"], str(self.target))
        self.assertEqual(config["remote-other"], foreign["remote-other"])
        self.assertEqual(config[self.identity], {"source": {"type": "local", "path": str(self.source)},
                                               "extensions": ["codescribe"]})
        before = self.target.read_bytes()
        self.assertEqual(self.install(), receipt)
        self.assertEqual(self.target.read_bytes(), before)

    def test_unregistered_marketplace_requires_native_registration_without_inventing_identity(self):
        with self.assertRaisesRegex(ValueError, "/extensions marketplace add"):
            self.install()
        self.assertFalse(self.target.exists())

    def test_install_appends_without_replacing_existing_extension_metadata(self):
        entry = {"source": {"type": "local", "path": str(self.source)},
                 "extensions": ["another"], "setting": "retained"}
        DEMUX.atomic_json(self.target, {self.identity: entry})
        self.install()
        self.assertEqual(DEMUX.read_json(self.target)[self.identity], dict(entry, extensions=["another", "codescribe"]))

    def test_corrupt_configuration_is_refused_and_preserved_byte_for_byte(self):
        self.directory.mkdir()
        for raw in ("{not json", "[]", "null", "42", json.dumps({self.identity: []}),
                    json.dumps({self.identity: {"source": {"type": "local", "path": str(self.source)}}}),
                    json.dumps({self.identity: {"source": {"type": "local", "path": str(self.source)}, "extensions": [1]}})):
            with self.subTest(raw=raw):
                self.target.write_text(raw)
                before = self.target.read_bytes()
                with self.assertRaises(ValueError):
                    self.install()
                self.assertEqual(self.target.read_bytes(), before)

    def test_conflicting_marketplace_source_is_refused_without_overwrite(self):
        DEMUX.atomic_json(self.target, {self.identity: {"source": {"type": "local", "path": "/unrelated"},
                                                       "extensions": ["another"]}})
        before = self.target.read_bytes()
        with self.assertRaisesRegex(ValueError, "conflict"):
            self.install()
        self.assertEqual(self.target.read_bytes(), before)

    def test_missing_payload_does_not_create_configuration(self):
        (self.source / ".claude-plugin" / "marketplace.json").unlink()
        with self.assertRaisesRegex(ValueError, "payload missing"):
            self.install()
        self.assertFalse(self.target.exists())

    def test_symlinked_configuration_is_refused_without_touching_its_target(self):
        outside = self.root / "unrelated.json"
        DEMUX.atomic_json(outside, {"unrelated": True})
        self.directory.mkdir()
        self.target.symlink_to(outside)
        before = outside.read_bytes()
        with self.assertRaises(ValueError):
            self.install()
        self.assertTrue(self.target.is_symlink())
        self.assertEqual(outside.read_bytes(), before)

    def test_concurrent_installations_preserve_both_local_sources(self):
        other_root = self.root / "second-bridge"
        other_source = other_root / "runtime" / "plugins"
        DEMUX.atomic_json(other_source / ".claude-plugin" / "marketplace.json", {"name": "codescribe", "plugins": []})
        DEMUX.atomic_json(self.target, {"foreign": {"retained": True},
                                      self.identity: {"source": {"type": "local", "path": str(self.source)}, "extensions": []},
                                      "native-second-marketplace": {"source": {"type": "local", "path": str(other_source)}, "extensions": []}})
        start = threading.Barrier(3)
        outcomes = queue.Queue()

        def install(root):
            start.wait(timeout=5)
            try:
                outcomes.put(MCP.install_extension(DEMUX, root, self.directory))
            except Exception as error:
                outcomes.put(error)

        threads = [threading.Thread(target=install, args=(root,)) for root in (self.root, other_root)]
        for thread in threads:
            thread.start()
        start.wait(timeout=5)
        for thread in threads:
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
        for _ in threads:
            result = outcomes.get(timeout=1)
            self.assertIsInstance(result, dict, result)
        config = DEMUX.read_json(self.target)
        self.assertEqual(config["foreign"], {"retained": True})
        sources = {entry["source"]["path"] for key, entry in config.items() if key != "foreign"}
        self.assertEqual(sources, {str(self.source), str(other_source)})
        self.assertEqual(len(config), 3)


class MCPProtocolTests(MailboxFixture):
    def start_server(self, override_bus=True):
        self.pending = []
        self.save()
        command = [sys.executable, str(SCRIPTS / "cs-bus"), "--mcp", "--bridge-home", str(self.root)]
        if override_bus:
            command += ["--bus", str(self.bus)]
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, bufsize=1)
        self.responses = queue.Queue()
        self.stderr = []

        def read_stdout():
            for line in self.process.stdout:
                self.responses.put(line)

        def read_stderr():
            self.stderr.extend(self.process.stderr)

        self.readers = [threading.Thread(target=read_stdout, daemon=True),
                        threading.Thread(target=read_stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        self.addCleanup(self.stop_server)
        self.addCleanup(self.detach_follower)

    def detach_follower(self):
        result = subprocess.run([sys.executable, str(SCRIPTS / "cs-bus"), "--detach",
                                 "--bridge-home", str(self.root), "--provider", "junie",
                                 "--session", self.session], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)

    def stop_server(self):
        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
            self.fail("MCP process did not exit after stdin EOF")
        finally:
            for reader in self.readers:
                reader.join(timeout=2)
            self.process.stdout.close()
            self.process.stderr.close()

    def send(self, method, identity=None, params=None):
        request = {"jsonrpc": "2.0", "method": method}
        if identity is not None:
            request["id"] = identity
        if params is not None:
            request["params"] = params
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()

    def response(self, identity, timeout=10):
        try:
            line = self.responses.get(timeout=timeout)
        except queue.Empty:
            self.fail(f"no MCP response for {identity}; stderr: {''.join(self.stderr)}")
        value = json.loads(line)
        self.assertEqual(value["jsonrpc"], "2.0")
        self.assertEqual(value["id"], identity, value)
        return value

    def tool(self, identity, name, arguments=None):
        self.send("tools/call", identity, {"name": f"codescribe_{name}",
                                          "arguments": {} if arguments is None else arguments})
        result = self.response(identity)["result"]
        self.assertFalse(result.get("isError"), result)
        self.assertEqual(result["content"][0]["type"], "text")
        return json.loads(result["content"][0]["text"])

    def connect(self):
        result = self.tool(10, "connect", {"session_id": self.session, "name": "Roman", "channel": "3"})
        self.assertEqual(result["kind"], "attach_receipt")
        self.assertEqual(result["provider"], "junie")
        self.assertEqual(result["provider_session_id"], self.session)
        self.assertEqual(result["wakeup"], "off")
        return result

    def start_active_wait(self, identity):
        self.send("tools/call", identity, {"name": "codescribe_wait", "arguments": {}})
        with self.assertRaises(queue.Empty):
            self.responses.get(timeout=0.2)
        self.send("tools/call", identity + 1, {"name": "codescribe_wait", "arguments": {}})
        result = self.response(identity + 1)["result"]
        self.assertTrue(result["isError"])
        self.assertIn("already active", result["content"][0]["text"])

    def test_initialize_and_list_advertise_all_seven_native_tools(self):
        self.start_server(override_bus=False)
        self.send("initialize", 1, {"protocolVersion": "2024-11-05", "capabilities": {},
                                    "clientInfo": {"name": "hermetic-test", "version": "1"}})
        result = self.response(1)["result"]
        self.assertEqual(result["protocolVersion"], "2024-11-05")
        self.assertEqual(result["serverInfo"]["name"], "codescribe")
        self.assertEqual(result["capabilities"]["tools"], {})
        self.send("notifications/initialized")
        self.send("tools/list", 2)
        tools = self.response(2)["result"]["tools"]
        self.assertEqual({tool["name"] for tool in tools}, {f"codescribe_{name}" for name in
                         ("connect", "wait", "read", "ack", "reply", "status", "detach")})
        for tool in tools:
            self.assertEqual(tool["inputSchema"]["type"], "object")
            self.assertFalse(tool["inputSchema"]["additionalProperties"])
        self.assert_not_acknowledged()

    def test_tools_call_requires_connection_and_survives_bad_method(self):
        self.start_server()
        self.send("tools/call", 1, {"name": "codescribe_read", "arguments": {}})
        result = self.response(1)["result"]
        self.assertTrue(result["isError"])
        self.assertIn("connect", result["content"][0]["text"])
        self.send("no-such-method", 2)
        self.assertEqual(self.response(2)["error"]["code"], -32601)
        self.send("ping", 3)
        self.assertEqual(self.response(3)["result"], {})
        self.assert_not_acknowledged()

    def test_connect_read_status_detach_use_real_bus_subprocesses(self):
        self.start_server()
        self.connect()
        result = self.tool(11, "read")
        self.assertEqual(result["deliveries"], [])
        self.assertEqual(result["read_delivery_ids"], [])
        status = self.tool(12, "status")
        self.assertEqual(status["lease_id"], self.lease)
        self.assertEqual(status["channel"], "3")
        self.assertEqual(status["backlog"], 0)
        detach = self.tool(13, "detach")
        self.assertEqual(detach["released_channels"], ["3"])
        self.send("tools/call", 14, {"name": "codescribe_read", "arguments": {}})
        self.assertTrue(self.response(14)["result"]["isError"])
        self.assert_not_acknowledged()

    def test_cancel_notification_releases_wait_without_ack_or_late_result(self):
        self.start_server()
        self.connect()
        self.start_active_wait(20)
        self.send("notifications/cancelled", params={"requestId": 20, "reason": "test cancellation"})
        self.send("ping", 30)
        self.assertEqual(self.response(30)["result"], {})
        with self.assertRaises(queue.Empty):
            self.responses.get(timeout=0.7)
        self.start_active_wait(40)
        self.send("notifications/cancelled", params={"requestId": 40})
        self.process.stdin.close()
        self.assertEqual(self.process.wait(timeout=5), 0)
        for reader in self.readers:
            reader.join(timeout=2)
        self.assertTrue(self.responses.empty(), "cancelled waits emitted a late response")
        self.assert_not_acknowledged()

    def test_eof_cancels_pending_wait_and_exits_without_ack(self):
        self.start_server()
        self.connect()
        self.start_active_wait(20)
        self.process.stdin.close()
        self.assertEqual(self.process.wait(timeout=5), 0)
        for reader in self.readers:
            reader.join(timeout=2)
        self.assertTrue(self.responses.empty(), "EOF produced a cancelled wait result")
        self.assert_not_acknowledged()


if __name__ == "__main__":
    unittest.main()
