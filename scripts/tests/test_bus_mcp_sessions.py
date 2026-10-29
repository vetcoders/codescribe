"""Host-issued scope and real native mailbox operations, isolated from live BUS."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import select
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from scripts.tests import test_bus_mcp as fixtures


SPEC = importlib.util.spec_from_file_location("bus_mcp_sessions_tests", fixtures.SCRIPTS / "bus-mcp-sessions.py")
SESSIONS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SESSIONS)
DEMUX, MCP = fixtures.DEMUX, fixtures.MCP


class SessionTests(fixtures.MailboxFixture):
    PROVIDER = "codex"

    def setUp(self):
        super().setUp()
        self.save(name="Ada")
        self.router = SESSIONS.SessionCapabilities(DEMUX, MCP, self.root, self.bus)
        self.token = self.issue(self.session)

    def issue(self, session, event="SessionStart", source="startup"):
        result = SESSIONS.host_hook(DEMUX, self.root, {"session_id": session, "hook_event_name": event, "source": source})
        return result["hookSpecificOutput"]["additionalContext"].split("session_capability=", 1)[1].split(" ", 1)[0]

    def call(self, name, values=None, token=None):
        return self.router.call("codescribe_" + name,
                                {"session_capability": self.token if token is None else token, **(values or {})},
                                threading.Event())

    def test_read_ack_and_cross_session_isolation_on_one_shared_server(self):
        other = "11111111-2222-4333-8444-666666666666"
        token = self.issue(other)
        result = self.call("read")
        self.assertEqual(result["read_delivery_ids"], self.ids[:1])
        with self.assertRaisesRegex(ValueError, "mailbox does not belong"):
            self.call("read", token=token)
        other_lease = DEMUX.lease_identifier("codex", other)
        DEMUX.atomic_json(self.root / "leases" / (other_lease + ".json"),
                          {"schema": DEMUX.LEASE_SCHEMA, "lease_id": other_lease, "provider": "codex",
                           "provider_session_id": other, "bus": str(self.bus), "pending": [], "name": "Ada"})
        self.assertEqual(self.call("read", token=token)["read_delivery_ids"], [])
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:1]}, token=token)
        self.call("ack", {"delivery_ids": result["read_delivery_ids"]})
        self.assertTrue((self.root / "acknowledgments" / self.lease / (self.ids[0] + ".json")).is_file())

    def test_capability_is_stable_for_compact_but_new_for_new_session(self):
        self.assertEqual(self.issue(self.session, source="compact"), self.token)
        self.assertEqual(self.issue(self.session, "UserPromptSubmit"), self.token)
        self.assertNotEqual(self.issue("11111111-2222-4333-8444-666666666666"), self.token)

    def test_raw_uuid_tamper_and_tool_selected_owner_are_refused(self):
        for token in (self.session, self.token[:-1] + ("0" if self.token[-1] != "0" else "1")):
            with self.assertRaises(PermissionError):
                self.call("read", token=token)
        with self.assertRaisesRegex(ValueError, "arguments"):
            self.call("connect", {"name": "Ada", "channel": "4", "session_id": self.session})
        self.assert_not_acknowledged()

    def test_end_revokes_access_without_removing_pending_or_ack(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        original = self.lease_path.read_bytes()
        SESSIONS.host_hook(DEMUX, self.root, {"session_id": self.session, "hook_event_name": "SessionEnd"})
        with self.assertRaisesRegex(PermissionError, "revoked"):
            self.call("read")
        self.assertEqual(self.lease_path.read_bytes(), original)
        resumed = self.issue(self.session)
        self.assertNotEqual(resumed, self.token)
        self.assertEqual(self.call("read", token=resumed)["read_delivery_ids"], self.ids[1:2])

    def test_mcp_restart_requires_another_full_read_before_ack(self):
        self.call("read")
        self.router = SESSIONS.SessionCapabilities(DEMUX, MCP, self.root, self.bus)
        with self.assertRaisesRegex(ValueError, "completely read"):
            self.call("ack", {"delivery_ids": self.ids[:1]})
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})

    def test_large_utf8_envelope_refuses_without_read_or_ack(self):
        self.pending[0]["text"] = "żółć 🐈" * 16000
        self.save()
        with self.assertRaises(ValueError):
            self.call("read")
        mailbox, _ = self.router.resolve(self.token)
        self.assertFalse(mailbox.read_ids)
        self.assert_not_acknowledged()

    def test_reply_requires_canonical_ack_and_text_defaults_to_no_audio(self):
        values = {"delivery_id": self.ids[0], "request_id": "a" * 24, "text": "Gotowe."}
        with self.assertRaises(PermissionError):
            self.call("reply", values)
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        mailbox, _ = self.router.resolve(self.token)
        response = {"kind": "agent_reply", "reply_id": "b" * 24, "provider": "codex",
                    "provider_session_id": self.session, "lease_id": self.lease}
        with patch.object(mailbox, "run", return_value=response) as run:
            receipt = self.call("reply", values)
            self.assertEqual(receipt["state"], "published")
            self.assertIn("--text-only", run.call_args.args[0])
            self.assertFalse(receipt["speech_attempted"])
            self.assertFalse(receipt["spoken"])
            self.assertEqual(self.call("reply", values), receipt)
            self.assertEqual(run.call_count, 1)
        # A new transport must return the same durable receipt without speech.
        self.router = SESSIONS.SessionCapabilities(DEMUX, MCP, self.root, self.bus)
        mailbox, _ = self.router.resolve(self.token)
        with patch.object(mailbox, "run", side_effect=AssertionError("must not replay")):
            self.assertEqual(self.call("reply", values), receipt)

    def test_uncertain_speech_never_replays_and_content_mismatch_is_refused(self):
        self.call("read")
        self.call("ack", {"delivery_ids": self.ids[:1]})
        mailbox, _ = self.router.resolve(self.token)
        values = {"delivery_id": self.ids[0], "request_id": "c" * 24, "text": "Gotowe.", "speak": True}
        with patch.object(mailbox, "run", side_effect=subprocess.TimeoutExpired("helper", 90)) as run:
            receipt = self.call("reply", values)
            self.assertEqual(receipt["state"], "uncertain")
            self.assertEqual(self.call("reply", values), receipt)
            self.assertEqual(run.call_count, 1)
        with self.assertRaisesRegex(ValueError, "different content"):
            self.call("reply", {**values, "text": "Inna odpowiedź."})

    def test_real_stdio_server_uses_host_scope_not_tool_uuid(self):
        requests = [{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}},
                    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                    {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                        "name": "codescribe_read", "arguments": {"session_capability": self.token}}}]
        # Keep stdin open until the tool answers: EOF intentionally cancels work.
        process = subprocess.Popen([sys.executable, str(fixtures.SCRIPTS / "cs-bus"), "--codex-mcp",
                                    "--bridge-home", str(self.root), "--bus", str(self.bus)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        self.addCleanup(process.stderr.close)
        self.addCleanup(process.stdout.close)
        for request in requests:
            process.stdin.write(json.dumps(request) + "\n")
            process.stdin.flush()
            self.assertTrue(select.select([process.stdout], [], [], 10)[0], "MCP response timed out")
            row = json.loads(process.stdout.readline())
            self.assertEqual(row["id"], request["id"], row)
            if request["id"] == 2:
                connect = next(t for t in row["result"]["tools"] if t["name"] == "codescribe_connect")
                self.assertNotIn("session_id", connect["inputSchema"]["properties"])
            if request["id"] == 3:
                self.assertEqual(row["result"]["structuredContent"]["read_delivery_ids"], self.ids[:1])
        process.stdin.close()
        process.wait(timeout=5)
        self.assertEqual(process.returncode, 0, process.stderr.read())

    def test_real_connect_verifies_one_follower_and_detach_retains_history(self):
        fresh = "11111111-2222-4333-8444-777777777777"
        token = self.issue(fresh)
        self.addCleanup(lambda: self.call("detach", token=token))
        connected = self.call("connect", {"name": "Probe", "channel": "4"}, token=token)
        self.assertEqual(connected["provider_session_id"], fresh)
        self.assertEqual(connected["wakeup"], "codex-queue")
        status = self.call("status", token=token)
        self.assertTrue(status["follower_verified"], status)
        self.assertTrue(status["attached"], status)
        again = self.call("connect", {"name": "Probe", "channel": "4"}, token=token)
        self.assertEqual(again["follower_pid"], connected["follower_pid"])
        self.call("detach", token=token)
        status = self.call("status", token=token)
        self.assertTrue(status["lease_recorded"])
        self.assertFalse(status["attached"])
        self.assertFalse(status["channel_bound"])

    def test_stale_lease_and_other_provider_same_uuid_are_not_attached(self):
        DEMUX.atomic_json(self.root / DEMUX.AUDIENCE_BINDING_FILENAME,
                          {"schema": DEMUX.AUDIENCE_BINDING_SCHEMA, "bindings": {
                              "4": {"provider": "claude-code", "provider_session_id": self.session,
                                    "audience": "Ada", "bus": str(self.bus)}}})
        self.save(name="Ada", pid=os.getpid(), heartbeat_unix=time.time())
        status = self.call("status")
        self.assertTrue(status["lease_recorded"])
        self.assertTrue(status["heartbeat_fresh"])
        self.assertFalse(status["attached"])
        self.assertFalse(status["follower_verified"])
        self.assertIsNone(status["channel"])
        self.assertEqual(status["follower_problem"], "follower_identity_or_lease_mismatch")


class ProcessProbeTests(unittest.TestCase):
    def test_process_probe_failures_stay_distinct_and_fail_closed(self):
        cases = [(PermissionError(), "process_probe_permission_denied"),
                 (subprocess.TimeoutExpired("ps", 2), "process_probe_timeout"),
                 (UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid"), "process_probe_malformed")]
        for error, reason in cases:
            with patch("subprocess.run", side_effect=error):
                self.assertIsNone(DEMUX.process_identity(123))
                self.assertEqual(DEMUX.probe_process_identity(123)["error"], reason)
        for code, stdout, reason in [(1, "", "process_probe_exit"), (0, "bad", "process_probe_malformed"),
                                     (0, "Thu Oct 29 20:00:00 2026 Z helper", "process_not_running")]:
            with patch("subprocess.run", return_value=subprocess.CompletedProcess([], code, stdout, "")):
                self.assertEqual(DEMUX.probe_process_identity(123)["error"], reason)
                self.assertIsNone(DEMUX.process_identity(123))

    def test_ps_permission_exit_is_distinct_and_does_not_disclose_stderr(self):
        for message in ("Operation not permitted", "Permission denied"):
            result = subprocess.CompletedProcess([], 1, "", message + " private argv")
            with self.subTest(message=message), patch("subprocess.run", return_value=result):
                probe = DEMUX.probe_process_identity(123)
                self.assertIsNone(probe["identity"])
                self.assertEqual(probe["error"], "process_probe_permission_denied")
                self.assertNotIn("private argv", json.dumps(probe))

    def test_startup_timeout_names_unmet_stage_and_preserves_lease(self):
        for stage in ("lease_schema_not_ready", "lease_bus_not_ready",
                      "events_file_missing", "follower_identity_unverified"):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as temporary:
                root, lease = Path(temporary), "a" * 32
                bus = str(root / "bus.jsonl")
                state = {"schema": DEMUX.LEASE_SCHEMA, "pid": 123, "lease_id": lease,
                         "active": True, "bus": bus, "pending": [{"text": "retained"}]}
                if stage == "lease_schema_not_ready":
                    state["schema"] = "wrong"
                if stage == "lease_bus_not_ready":
                    state["bus"] = "different"
                lease_path = root / "leases" / (lease + ".json")
                DEMUX.atomic_json(lease_path, state)
                before = lease_path.read_bytes()
                _log, events = DEMUX.follower_paths(root, lease)
                events.parent.mkdir(parents=True, exist_ok=True)
                if stage != "events_file_missing":
                    events.touch()
                child = SimpleNamespace(pid=123, returncode=None, poll=lambda: None)
                with patch.object(DEMUX.time, "monotonic", side_effect=[0, 0, 6]), \
                        patch.object(DEMUX.time, "sleep"), \
                        patch.object(DEMUX, "verified_follower", return_value=False) as verify, \
                        patch.object(DEMUX, "probe_process_identity", return_value={"identity": {}, "error": None}):
                    with self.assertRaisesRegex(OSError, stage):
                        DEMUX.confirm_follower(root, lease, "session", bus, child, {})
                    self.assertEqual(verify.call_count, int(stage == "follower_identity_unverified"))
                self.assertEqual(lease_path.read_bytes(), before)


class MonitorIdentityTests(fixtures.MailboxFixture):
    def test_live_pid_without_recorded_identity_does_not_certify_monitor(self):
        observed = {"started": "Thu Oct 29 20:00:00 2026", "command": "test-owned monitor"}
        for recorded, expected in ((None, False), (observed, True)):
            with self.subTest(recorded=recorded):
                DEMUX.atomic_json(DEMUX.watch_record_path(self.root, self.lease),
                                  {"pid": 123, "process_identity": recorded})
                args = SimpleNamespace(bridge_home=self.root, provider=self.PROVIDER,
                                       session=self.session, lease_ttl=5, bus=self.bus)
                with patch.object(DEMUX, "emit") as emit, \
                        patch.object(DEMUX, "process_identity", return_value=recorded), \
                        patch.object(DEMUX, "process_is_alive", return_value=True):
                    self.assertEqual(DEMUX.status_command(args), 0)
                self.assertEqual(emit.call_args.args[0]["listener"]["watch_alive"], expected)


if __name__ == "__main__":
    unittest.main()
