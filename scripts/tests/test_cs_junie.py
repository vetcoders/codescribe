"""Junie ACP transport and Codescribe delivery boundaries, without model calls."""
import asyncio
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

SOURCE = Path(__file__).resolve().parents[1] / "cs-junie"
SPEC = importlib.util.spec_from_loader("cs_junie", importlib.machinery.SourceFileLoader("cs_junie", str(SOURCE)))
PLUGIN = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PLUGIN)

PEER = Path(__file__).with_name("fixtures") / "junie_acp_peer.py"


class AcpTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.text = []
        self.client = PLUGIN.AcpClient([sys.executable, str(PEER)], self.root,
                                      on_text=self.text.append)
        await self.client.start()
        self.addAsyncCleanup(self.client.close)

    async def test_initialization_uses_supported_protocol_and_no_fake_capabilities(self):
        result = await self.client.request("initialize", {
            "protocolVersion": 1, "clientCapabilities": {},
            "clientInfo": {"name": "codescribe", "version": "1"}})
        self.assertEqual(result["protocolVersion"], 1)

    async def test_real_session_identity_is_returned_by_agent(self):
        result = await self.client.request("session/new", {"cwd": str(self.root), "mcpServers": []})
        self.assertEqual(result["sessionId"], "peer-owned-session")

    async def test_complete_text_stream_and_permission_refusal_are_not_deadlocks(self):
        self.client.session = "peer-owned-session"
        result = await self.client.request("session/prompt", {
            "sessionId": self.client.session, "prompt": [{"type": "text", "text": "permission"}]})
        self.assertEqual(result["stopReason"], "end_turn")
        self.assertEqual("".join(self.text), "permission=cancelled; pełna odpowiedź")

    async def test_foreign_session_updates_are_not_spoken(self):
        self.client.session = "peer-owned-session"
        await self.client.request("session/prompt", {
            "sessionId": self.client.session, "prompt": [{"type": "text", "text": "foreign"}]})
        self.assertEqual("".join(self.text), "pełna odpowiedź")

    async def test_agent_exit_fails_waiting_request(self):
        with self.assertRaises(RuntimeError):
            await asyncio.wait_for(self.client.request("exit", {}), 3)

    async def test_protocol_error_does_not_become_success(self):
        with self.assertRaises(RuntimeError):
            await self.client.request("unknown", {})


class TurnTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        args = PLUGIN.parse_args(["--name", "junie", "--channel", "6", "--bridge-home", str(self.root)])
        self.plugin = PLUGIN.JuniePlugin(args)
        self.plugin.client.session = "real-session"
        self.plugin.client.request = AsyncMock(return_value={"stopReason": "end_turn"})
        self.payload = {"delivery_id": "b" * 24, "lease_id": "lease-12345678", "provider": "junie",
                        "provider_session_id": "real-session", "bus": str(self.root / "bus"), "text": "Iwo"}

    async def test_loaded_history_is_not_replayed_as_a_fresh_answer(self):
        with patch("builtins.print") as output:
            self.plugin._text("old conversation")
            self.assertEqual(self.plugin.answer, [])
            output.assert_not_called()
            self.plugin.in_turn = True
            self.plugin._text("fresh answer")
            self.assertEqual(self.plugin.answer, ["fresh answer"])

    async def test_provider_completion_without_agent_ack_stops_and_retains_evidence(self):
        with patch.object(PLUGIN, "emit"), patch("builtins.print"):
            with self.assertRaises(RuntimeError):
                await self.plugin.turn(self.payload)
        self.assertFalse((self.root / "acknowledgments").exists())
        receipt = PLUGIN.read_json(self.plugin.journal.path(self.payload))
        self.assertEqual(receipt["state"], "incomplete")
        self.assertEqual(receipt["envelope"], self.payload)

    async def test_cancelled_turn_is_not_completed_even_after_read_receipt(self):
        path = self.root / "acknowledgments" / self.payload["lease_id"] / (self.payload["delivery_id"] + ".json")
        PLUGIN.atomic_json(path, {**self.payload, "read_at": "now"})
        self.plugin.client.request.return_value = {"stopReason": "cancelled"}
        with patch.object(PLUGIN, "emit"), patch("builtins.print"):
            with self.assertRaises(RuntimeError):
                await self.plugin.turn(self.payload)
        self.assertEqual(PLUGIN.read_json(self.plugin.journal.path(self.payload))["state"], "incomplete")


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = {"delivery_id": "a" * 24, "lease_id": "lease-12345678",
                        "provider": "junie", "provider_session_id": "real-session",
                        "bus": str(self.root / "bus"), "text": "Iwo\n" * 5000,
                        "sender": {"name": "Founder"}, "kind": "message"}

    def test_prompt_preserves_complete_envelope_and_exact_ack_command(self):
        command = ["/path with spaces/cs-bus", "--provider", "junie", "--session", "real-session"]
        prompt = PLUGIN.delivery_prompt(self.payload, command)
        self.assertEqual(json.loads(prompt.split("Codescribe envelope:\n", 1)[1]), self.payload)
        self.assertIn("--ack " + self.payload["delivery_id"], prompt)
        self.assertIn("'/path with spaces/cs-bus'", prompt)
        self.assertFalse(PLUGIN.is_acknowledged(self.root, self.payload))

    def test_foreign_ack_is_not_receipt(self):
        path = self.root / "acknowledgments" / self.payload["lease_id"] / (self.payload["delivery_id"] + ".json")
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({**self.payload, "provider_session_id": "another-session", "read_at": "now"}))
        self.assertFalse(PLUGIN.is_acknowledged(self.root, self.payload))
        path.write_text(json.dumps({**self.payload, "read_at": "now"}))
        self.assertTrue(PLUGIN.is_acknowledged(self.root, self.payload))

    def test_five_identical_texts_remain_five_deliveries(self):
        journal = PLUGIN.DeliveryJournal(self.root)
        for number in range(5):
            payload = {**self.payload, "delivery_id": f"{number:024x}"}
            journal.begin(payload)
            journal.finish(payload, "completed", "answer")
        self.assertEqual(len(list(self.root.glob("*.json"))), 5)

    def test_interrupted_or_completed_delivery_is_never_reexecuted_automatically(self):
        journal = PLUGIN.DeliveryJournal(self.root)
        journal.begin(self.payload)
        with self.assertRaises(RuntimeError):
            PLUGIN.DeliveryJournal(self.root).begin(self.payload)
        journal.finish(self.payload, "completed", "answer")
        with self.assertRaises(RuntimeError):
            journal.begin(self.payload)

    def test_channel_and_name_validation_happen_before_starting_junie(self):
        for channel in ("0", "10", "6; touch x"):
            with self.assertRaises(SystemExit):
                PLUGIN.parse_args(["--name", "junie", "--channel", channel])


if __name__ == "__main__":
    unittest.main()
