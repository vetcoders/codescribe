"""Native provider metadata reaches the owned lease without changing deliveries."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SOURCE = Path(__file__).resolve().parents[1] / "bus-demux.py"
SPEC = importlib.util.spec_from_file_location("bus_runtime_model", SOURCE)
DEMUX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DEMUX
SPEC.loader.exec_module(DEMUX)

SESSION = "11111111-2222-4333-8444-555555555555"
FOREIGN = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"


class RuntimeModelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.root = self.home / "agent-bridge"
        self.bus = self.home / "bus.jsonl"
        self.bus.touch()

    def codex_file(self, *, session=SESSION, folder="2026/10/07"):
        path = self.home / ".codex/sessions" / folder / f"rollout-native-{SESSION}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        self.rows(path, {"type": "session_meta", "payload": {"id": session}})
        return path

    def claude_file(self, *, folder="project-a"):
        path = self.home / ".claude/projects" / folder / f"{SESSION}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        self.rows(path, {"type": "user", "sessionId": SESSION, "message": {"content": "fixture"}})
        return path

    def rows(self, path, *records, append=False):
        with path.open("a" if append else "w", encoding="utf-8") as stream:
            for record in records:
                stream.write(json.dumps(record) + "\n")

    def context(self, model, timestamp="2026-10-07T16:00:00Z"):
        return {"type": "turn_context", "timestamp": timestamp, "payload": {"model": model}}

    def assistant(self, model, *, session=SESSION):
        return {"type": "assistant", "sessionId": session,
                "timestamp": "2026-10-07T16:00:00Z", "message": {"model": model}}

    def reader(self, provider="codex", previous=None):
        reader = DEMUX.NativeProviderModelReader(provider, SESSION, home=self.home, previous=previous)
        reader.DISCOVERY_INTERVAL = 0
        reader.refresh()
        return reader

    def model(self, reader):
        return reader.projection.get("model")

    def lease(self, provider="codex"):
        lease = DEMUX.SessionLease(
            root=self.root, provider=provider, provider_session_id=SESSION, name="astra",
            bus=self.bus, requested_id=None, ttl_seconds=30, follow_from_end=False,
            provider_metadata_home=self.home)
        self.addCleanup(lease._release_lock)
        return lease

    def test_codex_uses_native_model_and_observes_switch_without_message_text(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        reader = self.reader()
        self.assertEqual(self.model(reader), "gpt-6.1-sol")
        self.rows(path, {"type": "event_msg", "payload": {"model": "invented", "text": "model"}},
                  self.context("gpt-6-astra", "2026-10-07T16:01:00Z"), append=True)
        reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6-astra")

    def test_foreign_codex_identity_and_ambiguous_sources_leave_model_unknown(self):
        path = self.codex_file(session=FOREIGN)
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        self.assertIsNone(self.model(self.reader()))
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        other = self.codex_file(folder="2026/10/06")
        self.rows(other, self.context("gpt-6-astra"), append=True)
        self.assertIsNone(self.model(self.reader()))

    def test_partial_append_waits_for_complete_record(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        reader = self.reader()
        encoded = json.dumps(self.context("gpt-6-astra", "2026-10-07T16:01:00Z"))
        with path.open("a") as stream:
            stream.write(encoded[:len(encoded)//2])
        reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6.1-sol")
        with path.open("a") as stream:
            stream.write(encoded[len(encoded)//2:] + "\n")
        reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6-astra")

    def test_large_unrelated_tail_eventually_finds_available_model(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        unrelated = {"type": "event_msg", "payload": {"text": "a" * 65536}}
        self.rows(path, *([unrelated] * 80), append=True)
        reader = self.reader()
        for _ in range(12):
            reader.refresh()
            if self.model(reader):
                break
        self.assertEqual(self.model(reader), "gpt-6.1-sol")

    def test_replacement_and_truncation_require_native_identity_again(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        reader = self.reader()
        replacement = path.with_suffix(".replacement")
        self.rows(replacement, {"type": "session_meta", "payload": {"id": FOREIGN}},
                  self.context("gpt-6-astra"))
        replacement.replace(path)
        reader.refresh()
        self.assertIsNone(self.model(reader))
        self.rows(path, {"type": "session_meta", "payload": {"id": SESSION}},
                  self.context("gpt-6-astra"))
        reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6-astra")

    def test_claude_accepts_only_same_session_real_assistant_models(self):
        path = self.claude_file()
        self.rows(path, self.assistant("claude-fable-5"), append=True)
        reader = self.reader("claude-code")
        self.assertEqual(self.model(reader), "claude-fable-5")
        self.rows(path, self.assistant("foreign-model", session=FOREIGN),
                  self.assistant("<synthetic>"), append=True)
        reader.refresh()
        self.assertEqual(self.model(reader), "claude-fable-5")

    def test_unavailable_native_source_and_other_provider_never_infer_model(self):
        self.assertIsNone(self.model(self.reader()))
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        self.assertIsNone(self.model(self.reader("grok")))

    def test_owned_lease_retains_model_cursor_and_pending_across_reattachment(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        lease = self.lease()
        lease.refresh_provider_model()
        envelope = {"delivery_id": "d" * 24, "kind": "message", "text": "fixture",
                    "lease_id": lease.lease_id, "provider": lease.provider,
                    "provider_session_id": SESSION}
        lease.pending[envelope["delivery_id"]] = envelope
        lease.persist(active=False, cursor=0, sequence=17)
        initial = json.loads(lease.path.read_text())
        self.assertEqual(initial["model"], "gpt-6.1-sol")
        lease._release_lock()
        restored = self.lease()
        restored.refresh_provider_model()
        restored.persist(active=False)
        saved = json.loads(restored.path.read_text())
        self.assertEqual(saved["model"], "gpt-6.1-sol")
        self.assertEqual(saved["lease_id"], initial["lease_id"])
        self.assertEqual(saved["pending"], [envelope])
        self.assertEqual(saved["last_sequence"], 17)

    def test_native_metadata_failure_does_not_block_owned_lease_persistence(self):
        path = self.codex_file(session=FOREIGN)
        self.rows(path, self.context("untrusted"), append=True)
        lease = self.lease()
        lease.refresh_provider_model()
        lease.persist(active=True, cursor=0, sequence=23)
        saved = json.loads(lease.path.read_text())
        self.assertNotIn("model", saved)
        self.assertEqual(saved["last_sequence"], 23)
        self.assertTrue(saved["active"])

    def test_large_native_file_is_not_rescanned_on_unchanged_heartbeat_or_restore(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        unrelated = {"type": "event_msg", "payload": {"text": "a" * 65536}}
        self.rows(path, *([unrelated] * 80), append=True)
        reader = self.reader()
        for _ in range(12):
            reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6.1-sol")
        fdopen = DEMUX.os.fdopen
        reads = []

        class CountedFile:
            def __init__(self, handle):
                self.handle = handle

            def __enter__(self):
                self.handle.__enter__()
                return self

            def __exit__(self, *args):
                return self.handle.__exit__(*args)

            def __getattr__(self, name):
                return getattr(self.handle, name)

            def read(self, *args):
                data = self.handle.read(*args)
                reads.append(len(data))
                return data

            def readline(self, *args):
                data = self.handle.readline(*args)
                reads.append(len(data))
                return data

        with mock.patch.object(DEMUX.os, "fdopen", side_effect=lambda *a, **kw: CountedFile(fdopen(*a, **kw))):
            for _ in range(10):
                reader.refresh()
            restored = self.reader(previous=reader.projection)
        self.assertEqual(self.model(restored), "gpt-6.1-sol")
        self.assertLess(sum(reads), 32768, "unchanged/restore reads must not scale with the 5 MiB transcript")

    def test_truncate_and_regrow_same_inode_updates_model(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        reader = self.reader()
        inode = path.stat().st_ino
        self.rows(path, {"type": "session_meta", "payload": {"id": SESSION}},
                  self.context("gpt-6-astra"),
                  {"type": "event_msg", "payload": {"text": "new generation" * 100}})
        self.assertEqual(path.stat().st_ino, inode)
        reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6-astra")

    def test_claude_identity_after_large_unrelated_prefix_is_eventually_found(self):
        path = self.claude_file()
        unrelated = {"type": "progress", "message": "a" * 65536}
        self.rows(path, *([unrelated] * 48), self.assistant("claude-fable-5"))
        reader = self.reader("claude-code")
        for _ in range(8):
            reader.refresh()
        self.assertEqual(self.model(reader), "claude-fable-5")

    def test_model_does_not_publish_until_reader_catches_latest_append(self):
        path = self.codex_file()
        self.rows(path, self.context("gpt-6.1-sol"), append=True)
        reader = self.reader()
        unrelated = {"type": "event_msg", "payload": {"text": "a" * 65536}}
        self.rows(path, self.context("gpt-6-astra"), *([unrelated] * 48),
                  self.context("gpt-6-luna"), append=True)
        reader.refresh()
        self.assertIsNone(self.model(reader))
        for _ in range(8):
            reader.refresh()
        self.assertEqual(self.model(reader), "gpt-6-luna")


if __name__ == "__main__":
    unittest.main()
