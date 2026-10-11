"""Synthetic-only boundaries and cleanup; no production data or network."""
import importlib.util
import io
import json
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

SOURCE = Path(__file__).resolve().parents[1] / "mcp-synthetic-ping.py"
SPEC = importlib.util.spec_from_file_location("synthetic_ping_test", SOURCE)
PING = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PING)


class SyntheticPingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.session, self.identity, self.bus = PING.fixture(self.root)
        self.mailbox = PING.PROBE.Mailbox("fixture", self.session, self.root, self.bus, self.root)

    def test_one_synthetic_message_and_no_production_owner(self):
        result = self.mailbox.call("read_pending", {})
        self.assertEqual(len(result["events"]), 1)
        delivery = result["events"][0]["delivery"]
        self.assertEqual(delivery["text"], "Miksa, ping")
        self.assertEqual(delivery["provider"], "fixture")
        self.assertEqual(delivery["bus"], str(self.bus))
        self.assertNotIn("channel", delivery)
        self.assertFalse((self.root / "wakeups").exists())

    def test_ack_never_calls_native_queue(self):
        self.mailbox.call("read_pending", {})
        with patch.object(PING.D, "withdraw_acknowledged_queue", side_effect=AssertionError("queue forbidden")):
            receipt = self.mailbox.call("ack", {"request_id": self.identity})
        self.assertFalse(receipt["native_queue_settled"])

    def test_path_arguments_and_changed_local_path_refused(self):
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {"bus": "/outside/bus.jsonl"})
        self.mailbox.args.bus = self.root.parent / "outside.jsonl"
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {})

    def test_symlink_escape_refused_before_read(self):
        with tempfile.TemporaryDirectory() as other:
            outside = Path(other) / "synthetic-sentinel"
            outside.write_text("synthetic-only sentinel")
            (self.root / "escape").symlink_to(outside)
            with patch.object(PING.D, "read_pending_command") as read, self.assertRaises(ValueError):
                self.mailbox.call("read_pending", {})
            read.assert_not_called()
            self.assertEqual(outside.read_text(), "synthetic-only sentinel")

    def test_marker_owner_tamper_refused(self):
        PING.D.atomic_json(self.root / "fixture.json", {
            "schema": "codescribe.synthetic-probe.v1", "session": "foreign", "lease_id": self.mailbox.lease})
        with self.assertRaises(ValueError):
            self.mailbox.call("read_pending", {})

    def test_timeout_cleans_only_owned_fixture(self):
        created = []
        def timed_out(root, *_args):
            created.append(root)
            self.assertTrue(root.exists())
            raise subprocess.TimeoutExpired("fixture", 0.05)
        with patch.object(PING, "pinned_publisher", return_value=PING.PYTHON), \
                patch.object(PING, "exchange", side_effect=timed_out), self.assertRaises(subprocess.TimeoutExpired):
            PING.run(0.05)
        self.assertEqual(len(created), 1)
        self.assertFalse(created[0].exists())
        self.assertTrue(self.root.exists())

    def test_deadline_kills_owned_process_group_without_retry(self):
        child = MagicMock(pid=424242)
        child.communicate.side_effect = subprocess.TimeoutExpired("fixture", 0.05)
        with patch.object(PING.subprocess, "Popen", return_value=child) as spawn, \
                patch.object(PING.os, "killpg") as kill, self.assertRaises(subprocess.TimeoutExpired):
            PING.exchange(self.root, self.session, self.identity, self.bus, PING.PYTHON, 0.05)
        spawn.assert_called_once()
        kill.assert_called_once_with(424242, signal.SIGKILL)
        child.wait.assert_called_once_with(timeout=2)
        self.assertEqual(spawn.call_args.kwargs["env"]["HOME"], str(self.root))
        self.assertNotIn("OPENAI_API_KEY", spawn.call_args.kwargs["env"])
        self.assertEqual(spawn.call_args.kwargs["env"]["PATH"], str(PING.PYTHON.parent))

    def test_timeout_bounds_and_no_unsandboxed_mode(self):
        for timeout in [0, 31]:
            with self.assertRaises(ValueError):
                PING.run(timeout)
        with patch.object(PING.sys, "platform", "linux"), patch.object(PING.subprocess, "Popen") as spawn:
            with self.assertRaises(RuntimeError):
                PING.exchange(self.root, self.session, self.identity, self.bus, PING.PYTHON, 1)
        spawn.assert_not_called()

    def test_service_timeout_cleans_fixture_and_uses_no_local_exchange(self):
        created = []
        def timed_out(root, *_args):
            created.append(root)
            raise subprocess.TimeoutExpired("mock service", 0.05)
        with patch.object(PING, "pinned_publisher", return_value=PING.PYTHON), \
                patch.object(PING, "serve", side_effect=timed_out), \
                patch.object(PING, "exchange", side_effect=AssertionError("local ping forbidden")), \
                self.assertRaises(subprocess.TimeoutExpired):
            PING.run(0.05, service=True)
        self.assertEqual(len(created), 1)
        self.assertFalse(created[0].exists())
        self.assertTrue(self.root.exists())

    def test_service_always_stops_owned_child_after_relay_failure(self):
        child = MagicMock(pid=424242)
        with patch.object(PING, "start_child", return_value=child) as spawn, \
                patch.object(PING, "relay", side_effect=BrokenPipeError), \
                patch.object(PING.os, "killpg") as kill, self.assertRaises(BrokenPipeError):
            PING.serve(self.root, self.session, self.bus, PING.PYTHON, 0.05)
        spawn.assert_called_once_with(self.root, self.session, self.bus, PING.PYTHON, text=False)
        kill.assert_called_once_with(424242, signal.SIGKILL)
        child.wait.assert_called_once_with(timeout=2)
        child.stdout.close.assert_called_once()

    def test_frame_budget_refuses_incomplete_oversized_and_excess_frames(self):
        budget = PING.FrameBudget()
        self.assertEqual(budget.feed(b'{"id":'), b"")
        self.assertEqual(budget.feed(b'1}\n'), b'{"id":1}\n')
        budget.finish()
        for data in (b"x", b"x" * PING.PROBE.LIMIT, b"{}\n" * (PING.MAX_FRAMES + 1)):
            with self.subTest(size=len(data)), self.assertRaises(ValueError):
                budget = PING.FrameBudget()
                budget.feed(data)
                budget.finish()
        with patch.object(PING, "MAX_TRAFFIC", 5), self.assertRaises(ValueError):
            PING.FrameBudget().feed(b"{}\n{}\n")

    def test_publisher_is_fixed_to_os_user_not_environment_or_path(self):
        candidate = self.root / ".cargo" / "bin" / "codescribe"
        candidate.parent.mkdir(parents=True)
        candidate.write_text("mock publisher, never executed")
        candidate.chmod(0o700)
        account = MagicMock(pw_dir=str(self.root))
        with patch.object(PING.pwd, "getpwuid", return_value=account), \
                patch.dict(PING.os.environ, {"HOME": "/foreign", "PATH": "/foreign"}), \
                patch.object(PING.D, "reply_publisher_command", side_effect=AssertionError("PATH lookup forbidden")):
            self.assertEqual(PING.pinned_publisher(), candidate)
        candidate.unlink()
        candidate.symlink_to(PING.PYTHON)
        with patch.object(PING.pwd, "getpwuid", return_value=account), self.assertRaises(RuntimeError):
            PING.pinned_publisher()

    def test_service_cli_has_no_non_rpc_stdout(self):
        output = io.StringIO()
        with patch.object(PING.sys, "argv", ["synthetic", "--serve", "--timeout", "15"]), \
                patch.object(PING, "run", return_value=None) as run, \
                patch.object(PING.sys, "stdout", output):
            self.assertEqual(PING.main(), 0)
        run.assert_called_once_with(15, service=True)
        self.assertEqual(output.getvalue(), "")

    def test_relay_mock_forwards_partial_writes_and_restores_stdio_flags(self):
        child = MagicMock()
        child.stdin.fileno.return_value = 20
        child.stdout.fileno.return_value = 21
        child.stderr.fileno.return_value = 22
        child.stdin.closed = False
        child.wait.return_value = 0
        incoming, outgoing = MagicMock(), MagicMock()
        incoming.fileno.return_value, outgoing.fileno.return_value = 10, 11
        selector = MagicMock()
        selector.__enter__.return_value = selector
        selector.select.side_effect = [
            [(SimpleNamespace(fd=fd), 0) for fd in wave]
            for wave in ([10, 21], [20, 11], [20, 11], [10, 21, 22])]
        reads = {10: [b"{}\n", b""], 21: [b"{}\n", b""], 22: [b""]}
        written = {20: bytearray(), 11: bytearray()}
        def write(fd, data):
            count = min(len(data), 1 if not written[fd] else 2)
            written[fd].extend(data[:count])
            return count
        with patch.object(PING.selectors, "DefaultSelector", return_value=selector), \
                patch.object(PING.os, "get_blocking", return_value=True), \
                patch.object(PING.os, "set_blocking") as flags, \
                patch.object(PING.os, "read", side_effect=lambda fd, _: reads[fd].pop(0)), \
                patch.object(PING.os, "write", side_effect=write), \
                patch.object(PING.time, "monotonic", return_value=0):
            PING.relay(child, 15, incoming, outgoing)
        self.assertEqual(written, {20: b"{}\n", 11: b"{}\n"})
        self.assertIn(unittest.mock.call(10, True), flags.call_args_list)
        self.assertIn(unittest.mock.call(11, True), flags.call_args_list)
        child.stdin.close.assert_called_once()

    def test_relay_mock_idle_deadline_and_diagnostics_refuse(self):
        child = MagicMock()
        child.stdin.fileno.return_value = 20
        child.stdout.fileno.return_value = 21
        child.stderr.fileno.return_value = 22
        child.stdin.closed = False
        incoming, outgoing = MagicMock(), MagicMock()
        incoming.fileno.return_value, outgoing.fileno.return_value = 10, 11
        selector = MagicMock()
        selector.__enter__.return_value = selector
        selector.select.return_value = []
        with patch.object(PING.selectors, "DefaultSelector", return_value=selector), \
                patch.object(PING.os, "get_blocking", return_value=True), \
                patch.object(PING.os, "set_blocking"), \
                patch.object(PING.time, "monotonic", side_effect=[0, 0, 16]), \
                self.assertRaises(subprocess.TimeoutExpired):
            PING.relay(child, 15, incoming, outgoing)
        selector.select.return_value = [(SimpleNamespace(fd=22), 0)]
        with patch.object(PING.selectors, "DefaultSelector", return_value=selector), \
                patch.object(PING.os, "get_blocking", return_value=True), \
                patch.object(PING.os, "set_blocking"), \
                patch.object(PING.time, "monotonic", return_value=0), \
                patch.object(PING.os, "read", return_value=b"private diagnostic"), \
                self.assertRaisesRegex(RuntimeError, "emitted diagnostics"):
            PING.relay(child, 15, incoming, outgoing)

    def test_synthetic_dispatch_read_ack_empty_reply_with_mock_publisher(self):
        def call(identity, name, arguments):
            response = PING.PROBE.dispatch(self.mailbox, {"jsonrpc": "2.0", "id": identity,
                "method": "tools/call", "params": {"name": name, "arguments": arguments}})
            self.assertFalse(response["result"].get("isError"), response)
            return json.loads(response["result"]["content"][0]["text"])
        read = call(1, "read_pending", {})
        identity = read["events"][0]["request_id"]
        self.assertEqual(read["events"][0]["delivery"]["text"], "Miksa, ping")
        call(2, "ack", {"request_id": identity})
        self.assertEqual(call(3, "read_pending", {})["events"], [])
        with patch.object(PING.D, "publish_reply_event", return_value={
                "stream_dev": 0, "stream_inode": 1, "stream_id": "mock", "offset": 0, "length": 1}) as publish, \
                patch.object(PING.subprocess, "Popen", side_effect=AssertionError("runtime forbidden")):
            receipt = call(4, "reply", {"request_id": identity, "text": "pong"})
        self.assertEqual(receipt["state"], "published")
        self.assertFalse(receipt["spoken"])
        self.assertEqual(publish.call_args.args[0], self.bus)
        self.assertEqual(publish.call_args.args[1]["text"], "pong")
        self.assertFalse((self.root / "wakeups").exists())


if __name__ == "__main__":
    unittest.main()
