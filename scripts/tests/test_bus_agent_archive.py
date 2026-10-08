"""Founder archive action releases only a dead exact owner, preserving history."""
import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HELPER = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_agent_archive", HELPER)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class AgentArchiveTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve() / "bridge"
        self.bus = Path(temporary.name).resolve() / "messages.jsonl"
        self.bus.write_text('{"text":"Iwo Iwo Iwo Iwo Iwo"}\n')
        self.provider, self.session, self.channel = "codex", "dead-session", "4"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        self.lease = self.root / "leases" / f"{self.lease_id}.json"
        DEMUX.atomic_json(self.lease, {
            "schema": DEMUX.LEASE_SCHEMA, "provider": self.provider,
            "provider_session_id": self.session, "lease_id": self.lease_id,
            "name": "adam", "bus": str(self.bus), "cursor": 371,
            "pending": [{"delivery_id": "1" * 24, "text": "Unread take"}],
        })
        DEMUX.write_channel_binding(self.root, self.channel, "adam", self.provider, self.session)
        self.binding = self.root / DEMUX.AUDIENCE_BINDING_FILENAME
        self.archive = self.root / "archives" / f"{self.lease_id}-{self.channel}.json"
        self.arguments = argparse.Namespace(bridge_home=self.root, provider=self.provider,
            session=self.session, lease=self.lease_id, archive_agent=self.channel, bus=self.bus)
        self.original_lease, self.original_bus = self.lease.read_bytes(), self.bus.read_bytes()

    def assert_history_preserved(self):
        self.assertEqual(self.lease.read_bytes(), self.original_lease)
        self.assertEqual(self.bus.read_bytes(), self.original_bus)
        self.assertFalse((self.root / "acknowledgments").exists())

    def test_cli_archives_dead_owner_and_frees_channel_without_consuming_history(self):
        command = [sys.executable, str(HELPER), "--archive-agent", self.channel,
            "--provider", self.provider, "--session", self.session, "--lease", self.lease_id,
            "--bus", str(self.bus), "--bridge-home", str(self.root)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        receipt = json.loads(result.stdout)
        self.assertTrue(receipt["released"])
        self.assertEqual(receipt["name"], "adam")
        self.assertNotIn(self.channel, DEMUX.read_json(self.binding)["bindings"])
        self.assertEqual(DEMUX.read_json(self.archive), receipt)
        self.assert_history_preserved()
        # The exact operation is repeatable; a new session can use the freed digit.
        self.assertEqual(DEMUX.archive_agent(self.arguments), receipt)
        DEMUX.write_channel_binding(self.root, self.channel, "new-agent", "codex", "new-session")
        before = self.binding.read_bytes()
        with self.assertRaisesRegex(ValueError, "owner changed"):
            DEMUX.archive_agent(self.arguments)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assert_history_preserved()

    def test_stale_click_does_not_archive_a_replacement_owner(self):
        with DEMUX.channel_bindings(self.root) as bindings:
            bindings[self.channel]["provider_session_id"] = "replacement"
        before = self.binding.read_bytes()
        with self.assertRaisesRegex(ValueError, "owner changed"):
            DEMUX.archive_agent(self.arguments)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertFalse(self.archive.exists())

    def test_live_pid_and_exclusive_lease_both_refuse_without_signaling(self):
        before = self.binding.read_bytes()
        state = DEMUX.read_json(self.lease)
        state["pid"] = os.getpid()
        DEMUX.atomic_json(self.lease, state)
        with mock.patch.object(os, "kill", wraps=os.kill) as signaling:
            with self.assertRaisesRegex(ValueError, "alive"):
                DEMUX.archive_agent(self.arguments)
            self.assertTrue(all(call.args[1] == 0 for call in signaling.call_args_list))
        self.lease.write_bytes(self.original_lease)
        with open(self.lease.with_suffix(".lock"), "a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(ValueError, "running"):
                DEMUX.archive_agent(self.arguments)
        self.assertEqual(self.binding.read_bytes(), before)
        self.assertFalse(self.archive.exists())
        self.assert_history_preserved()

    def test_unreadable_or_wrong_bus_mailbox_keeps_channel(self):
        before = self.binding.read_bytes()
        for content in [b"{broken", json.dumps({**DEMUX.read_json(self.lease), "bus": "/wrong"}).encode()]:
            self.lease.write_bytes(content)
            with self.assertRaisesRegex(ValueError, "mailbox"):
                DEMUX.archive_agent(self.arguments)
            self.assertEqual(self.binding.read_bytes(), before)
            self.assertFalse(self.archive.exists())

    def test_metadata_failure_and_binding_failure_preserve_ownership(self):
        before = self.binding.read_bytes()
        original = DEMUX.atomic_json
        for rejected in [self.archive, self.binding]:
            def publish(path, value):
                if path == rejected:
                    raise OSError("fixture write refused")
                return original(path, value)
            with mock.patch.object(DEMUX, "atomic_json", side_effect=publish):
                with self.assertRaises(OSError):
                    DEMUX.archive_agent(self.arguments)
            self.assertEqual(self.binding.read_bytes(), before)
            self.assert_history_preserved()

    def test_only_selected_channel_is_released(self):
        DEMUX.write_channel_binding(self.root, "7", "adam", self.provider, self.session)
        DEMUX.archive_agent(self.arguments)
        self.assertIn("7", DEMUX.read_json(self.binding)["bindings"])
        self.assert_history_preserved()

    def test_contradictory_commands_refuse_before_any_side_effect(self):
        before = self.binding.read_bytes()
        for extra in [["--version"], ["--detach"], ["--ack", "1" * 24]]:
            result = subprocess.run([sys.executable, str(HELPER), "--archive-agent", self.channel,
                "--provider", self.provider, "--session", self.session, "--lease", self.lease_id,
                "--bus", str(self.bus), "--bridge-home", str(self.root), *extra],
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertEqual(self.binding.read_bytes(), before)
            self.assertFalse(self.archive.exists())
        self.assert_history_preserved()


if __name__ == "__main__":
    unittest.main()
