"""Bell watcher acceptance: follower exposes a per-lease bell.jsonl."""
import argparse
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "bus_bell_test", Path(__file__).resolve().parents[1] / "bus-demux.py"
)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)


class BellTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.provider = "kimi"
        self.session = "kimi-test-thread"
        self.lease_id = DEMUX.lease_identifier(self.provider, self.session)
        self.ids = [f"{index:024x}" for index in range(1, 6)]
        self.pending = [self.envelope(identity) for identity in self.ids]
        self._write_lease(self.provider, self.session, self.pending)

    def _write_lease(self, provider, session, pending):
        lease_id = DEMUX.lease_identifier(provider, session)
        DEMUX.atomic_json(self.root / "leases" / f"{lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": lease_id,
            "provider": provider, "provider_session_id": session,
            "pending": pending,
        })

    def envelope(self, identity, provider=None, session=None):
        provider = provider or self.provider
        session = session or self.session
        lease_id = DEMUX.lease_identifier(provider, session)
        return {
            "kind": "seal", "delivery_id": identity, "lease_id": lease_id,
            "provider": provider, "provider_session_id": session,
            "delivery_owner": {"lease_id": lease_id, "provider": provider,
                               "provider_session_id": session},
            "audience": "lena", "text": "Lena, pięć Iwo; `echo $HOME` nie jest kodem.",
            "state_change_allowed": False, "coverage": "refused",
            "wav": "/private/audio.wav", "sample_start": 123, "sample_end": 456,
        }

    def wake(self, provider=None, session=None, channel="2"):
        provider = provider or self.provider
        session = session or self.session
        return DEMUX.BellWakeup(self.root, provider, session, channel)

    def result(self, identity, provider=None, session=None):
        provider = provider or self.provider
        session = session or self.session
        lease_id = DEMUX.lease_identifier(provider, session)
        return DEMUX.read_json(self.root / "wakeups" / lease_id / f"{identity}.json")

    def bell_path(self, provider=None, session=None):
        provider = provider or self.provider
        session = session or self.session
        lease_id = DEMUX.lease_identifier(provider, session)
        return self.root / "runtime" / "followers" / f"{lease_id}.bell.jsonl"

    def bell_lines(self, provider=None, session=None):
        path = self.bell_path(provider, session)
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]

    def prepare_ack_state(self, provider=None, session=None):
        provider = provider or self.provider
        session = session or self.session
        lease_id = DEMUX.lease_identifier(provider, session)
        path = self.root / "leases" / f"{lease_id}.json"
        state = DEMUX.read_json(path)
        state["bus"] = str(self.root / "bus.jsonl")
        state["wakeup_configuration"] = {"wakeup": "bell", "on_seal": None}
        DEMUX.atomic_json(path, state)

    def ack_args(self, identity, provider=None, session=None):
        provider = provider or self.provider
        session = session or self.session
        return argparse.Namespace(ack=[identity], provider=provider, session=session,
                                  bridge_home=self.root, bus=self.root / "bus.jsonl",
                                  bus_overridden=False)

    def test_cli_rejects_bell_for_codex(self):
        result = subprocess.run([
            sys.executable, SPEC.origin, "--provider", "codex",
            "--session", self.session, "--wakeup", "bell",
            "--attach", "--channel", "2", "--name", "lena",
            "--bridge-home", str(self.root),
        ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("bell", result.stderr)

    def test_cli_rejects_bell_with_on_seal(self):
        result = subprocess.run([
            sys.executable, SPEC.origin, "--provider", self.provider,
            "--session", self.session, "--wakeup", "bell",
            "--on-seal", "echo hook",
            "--attach", "--channel", "2", "--name", "lena",
            "--bridge-home", str(self.root),
        ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn("bell", result.stderr)

    def test_effective_wakeup_auto_for_managed_follower_returns_bell(self):
        args = argparse.Namespace(
            wakeup="auto", provider=self.provider, session=self.session,
            attach=True, follower_channel="2", on_seal=None
        )
        self.assertEqual(DEMUX.effective_wakeup(args), "bell")

    def test_effective_wakeup_auto_without_managed_follower_returns_off(self):
        args = argparse.Namespace(
            wakeup="auto", provider=self.provider, session=self.session,
            attach=False, follower_channel=None, on_seal=None
        )
        self.assertEqual(DEMUX.effective_wakeup(args), "off")

    def test_effective_wakeup_explicit_bell(self):
        args = argparse.Namespace(
            wakeup="bell", provider=self.provider, session=self.session,
            attach=False, follower_channel=None, on_seal=None
        )
        self.assertEqual(DEMUX.effective_wakeup(args), "bell")

    def test_seal_writes_bell_receipt_for_kimi(self):
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        receipt = self.result(self.ids[0])
        self.assertEqual(receipt.get("schema"), "codescribe.bell.receipt.v1")
        self.assertEqual(receipt.get("disposition"), "bell_posted")
        self.assertEqual(receipt.get("provider"), self.provider)
        self.assertEqual(receipt.get("provider_session_id"), self.session)
        self.assertEqual(receipt.get("delivery_id"), self.ids[0])
        lines = self.bell_lines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(lines[0]["kind"], "seal")
        self.assertEqual(lines[0]["delivery_id"], self.ids[0])
        self.assertIn("emitted_at", lines[0])
        self.assertEqual(lines[0]["text"], self.pending[0]["text"])

    def test_typed_message_uses_bell(self):
        payload = dict(self.pending[0], kind="message", source="typed", state_change_allowed=True)
        for key in ("coverage", "wav", "sample_start", "sample_end"):
            payload.pop(key, None)
        self._write_lease(self.provider, self.session, self.pending + [payload])
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        receipt = self.result(self.ids[0])
        self.assertEqual(receipt.get("disposition"), "bell_posted")
        lines = self.bell_lines()
        self.assertEqual(len(lines), 1)
        self.assertEqual(lines[0]["kind"], "message")

    def test_draft_and_revised_ignored(self):
        draft = dict(self.pending[0], kind="draft")
        revised = dict(self.pending[0], kind="revised")
        wake = self.wake()
        wake.enqueue(draft)
        wake.enqueue(revised)
        wake.close(wait=True)
        self.assertFalse((self.root / "wakeups" / self.lease_id / f"{self.ids[0]}.json").exists())
        self.assertEqual(self.bell_lines(), [])

    def test_idempotent_enqueue(self):
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        self.assertEqual(len(self.bell_lines()), 1)

    def test_five_seals_appended_once(self):
        wake = self.wake()
        for payload in self.pending:
            wake.enqueue(payload)
            wake.enqueue(payload)
        wake.close(wait=True)
        lines = self.bell_lines()
        self.assertEqual(len(lines), 5)
        for line, identity in zip(lines, self.ids):
            self.assertEqual(line["delivery_id"], identity)
            self.assertEqual(line["kind"], "seal")
        for identity in self.ids:
            self.assertEqual(self.result(identity)["disposition"], "bell_posted")
        resumed = self.wake()
        for payload in self.pending:
            resumed.enqueue(payload)
        resumed.close(wait=True)
        self.assertEqual(len(self.bell_lines()), 5)

    def test_long_text_truncated_to_500(self):
        payload = dict(self.pending[0], text="x" * 1000)
        self._write_lease(self.provider, self.session, [payload] + self.pending[1:])
        wake = self.wake()
        wake.enqueue(payload)
        wake.close(wait=True)
        lines = self.bell_lines()
        self.assertEqual(len(lines[0]["text"]), 500)

    def test_retry_wakeup_reposts_when_not_bell_posted(self):
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{self.ids[0]}.json", {
            "schema": "codescribe.bell.receipt.v1",
            "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "delivery_id": self.ids[0],
            "disposition": "requesting",
        })
        wake = self.wake()
        wake.enqueue(self.pending[0], retry=True)
        wake.close(wait=True)
        self.assertEqual(len(self.bell_lines()), 1)
        self.assertEqual(self.result(self.ids[0])["disposition"], "bell_posted")

    def test_retry_wakeup_skips_when_bell_posted(self):
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{self.ids[0]}.json", {
            "schema": "codescribe.bell.receipt.v1",
            "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "delivery_id": self.ids[0],
            "disposition": "bell_posted",
        })
        wake = self.wake()
        wake.enqueue(self.pending[0], retry=True)
        wake.close(wait=True)
        self.assertEqual(self.bell_lines(), [])

    def test_ack_before_sender_starts_suppresses_bell(self):
        self.prepare_ack_state()
        DEMUX.acknowledge_delivery(self.ack_args(self.ids[0]))
        wake = self.wake()
        wake.enqueue(self.pending[0])
        wake.close(wait=True)
        self.assertEqual(self.bell_lines(), [])

    def test_ack_reports_native_queue_settled_true(self):
        self.prepare_ack_state()
        identity = self.ids[0]
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{identity}.json", {
            "schema": "codescribe.bell.receipt.v1",
            "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "delivery_id": identity,
            "disposition": "bell_posted",
        })
        with patch.object(DEMUX, "emit") as emit:
            DEMUX.acknowledge_delivery(self.ack_args(identity))
        payload = emit.call_args.args[0]
        self.assertEqual(payload["kind"], "acknowledged")
        self.assertTrue(payload["native_queue_settled"])
        self.assertTrue(DEMUX.delivery_acknowledged(self.root, self.lease_id, identity))

    def test_status_reports_bell_wakeup(self):
        self.prepare_ack_state()
        args = argparse.Namespace(
            provider=self.provider, session=self.session, bridge_home=self.root,
            bus=self.root / "bus.jsonl", lease_ttl=DEMUX.DEFAULT_LEASE_TTL_SECONDS
        )
        with patch.object(DEMUX, "emit") as emit:
            DEMUX.status_command(args)
        payload = emit.call_args.args[0]
        self.assertEqual(payload["wakeup"], "bell")

    def test_attach_receipt_reports_bell_wakeup(self):
        bus = self.root / "channel.jsonl"
        bus.touch()
        command = [
            sys.executable, str(SPEC.origin), "--bus", str(bus),
            "--bridge-home", str(self.root), "--attach",
            "--channel", "2", "--name", "lena", "--provider", self.provider,
            "--session", "test-attached-thread",
        ]
        receipt = json.loads(subprocess.check_output(command, text=True))
        pid = receipt["follower_pid"]
        try:
            self.assertEqual(receipt["wakeup"], "bell")
        finally:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass

    def test_cli_retry_wakeup_reposts_when_not_bell_posted(self):
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id,
            "provider": self.provider, "provider_session_id": self.session,
            "pending": [self.pending[0]],
            "bus": str(self.root / "bus.jsonl"),
        })
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{self.ids[0]}.json", {
            "schema": "codescribe.bell.receipt.v1",
            "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "delivery_id": self.ids[0],
            "disposition": "requesting",
        })
        result = subprocess.run([
            sys.executable, SPEC.origin, "--provider", self.provider,
            "--session", self.session, "--retry-wakeup", self.ids[0],
            "--bridge-home", str(self.root),
        ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt["disposition"], "bell_posted")
        self.assertEqual(len(self.bell_lines()), 1)

    def test_cli_retry_wakeup_skips_when_bell_posted(self):
        DEMUX.atomic_json(self.root / "leases" / f"{self.lease_id}.json", {
            "schema": DEMUX.LEASE_SCHEMA, "lease_id": self.lease_id,
            "provider": self.provider, "provider_session_id": self.session,
            "pending": [self.pending[0]],
            "bus": str(self.root / "bus.jsonl"),
        })
        DEMUX.atomic_json(self.root / "wakeups" / self.lease_id / f"{self.ids[0]}.json", {
            "schema": "codescribe.bell.receipt.v1",
            "lease_id": self.lease_id, "provider": self.provider,
            "provider_session_id": self.session, "delivery_id": self.ids[0],
            "disposition": "bell_posted",
        })
        result = subprocess.run([
            sys.executable, SPEC.origin, "--provider", self.provider,
            "--session", self.session, "--retry-wakeup", self.ids[0],
            "--bridge-home", str(self.root),
        ], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.bell_lines(), [])

    def test_public_help_includes_bell(self):
        help_text = subprocess.check_output([sys.executable, SPEC.origin, "--help"], text=True)
        self.assertIn("bell", help_text)
        self.assertNotIn("kimi-bell", help_text)

    def test_bell_receipt_for_claude_code(self):
        provider = "claude-code"
        session = "claude-test-thread"
        lease_id = DEMUX.lease_identifier(provider, session)
        identity = self.ids[0]
        pending = [self.envelope(identity, provider=provider, session=session)]
        self._write_lease(provider, session, pending)
        wake = self.wake(provider=provider, session=session)
        wake.enqueue(pending[0])
        wake.close(wait=True)
        receipt = self.result(identity, provider=provider, session=session)
        self.assertEqual(receipt.get("schema"), "codescribe.bell.receipt.v1")
        self.assertEqual(receipt.get("disposition"), "bell_posted")
        self.assertEqual(receipt.get("provider"), provider)
        lines = self.bell_lines(provider=provider, session=session)
        self.assertEqual(len(lines), 1)
        self.assertEqual(lines[0]["delivery_id"], identity)
        self.assertEqual(lines[0]["kind"], "seal")

    def test_effective_wakeup_auto_returns_bell_for_claude_code_managed_follower(self):
        args = argparse.Namespace(
            wakeup="auto", provider="claude-code", session="claude-test-thread",
            attach=True, follower_channel="2", on_seal=None
        )
        self.assertEqual(DEMUX.effective_wakeup(args), "bell")

    def test_effective_wakeup_auto_returns_off_for_claude_code_without_managed_follower(self):
        args = argparse.Namespace(
            wakeup="auto", provider="claude-code", session="claude-test-thread",
            attach=False, follower_channel=None, on_seal=None
        )
        self.assertEqual(DEMUX.effective_wakeup(args), "off")


if __name__ == "__main__":
    unittest.main()
