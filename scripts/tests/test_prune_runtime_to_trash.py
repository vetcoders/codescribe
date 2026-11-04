#!/usr/bin/env python3
"""Hermetic tests for scripts/prune-runtime-to-trash.py.

Verifies:
1. Active generational journal segments and receipts (*.generations.json) are protected.
2. Active leases and their follower logs are protected.
3. Dead follower logs, old backup dirs, expired acknowledgments, and old sessions
   are moved into ~/.Trash with preserved relative paths.
4. Golden and pinned datasets are never touched.
5. Dry-run mode never mutates the filesystem.
"""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

SCRIPTS_DIR = Path(__file__).resolve().parents[1]
MODULE_PATH = SCRIPTS_DIR / "prune-runtime-to-trash.py"

spec = importlib.util.spec_from_file_location("prune_runtime_to_trash", MODULE_PATH)
if spec is None or spec.loader is None:
    raise ImportError(f"Cannot load module from {MODULE_PATH}")
prune_mod = importlib.util.module_from_spec(spec)
sys.modules["prune_runtime_to_trash"] = prune_mod
spec.loader.exec_module(prune_mod)


class TestPruneRuntimeToTrash(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.base = Path(self.temp_dir.name)
        self.codescribe_root = self.base / ".codescribe"
        self.trash_root = self.base / ".Trash"

        self.codescribe_root.mkdir(parents=True)
        self.trash_root.mkdir(parents=True)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def set_age(self, path: Path, days_ago: float) -> None:
        past = time.time() - (days_ago * 86400)
        os.utime(path, (past, past))

    def test_generations_segments_and_active_are_protected(self) -> None:
        """Generations manifest and all referenced segment paths must never be pruned."""
        events_dir = self.codescribe_root / "events"
        events_dir.mkdir(parents=True)

        seg1 = events_dir / "segment-0001.jsonl"
        seg1.write_text('{"event":"chunk1"}\n')
        self.set_age(seg1, 45)  # 45 days old

        active_bus = self.codescribe_root / "transcript-events.jsonl"
        active_bus.write_text('{"event":"chunk2"}\n')
        self.set_age(active_bus, 45)

        manifest = self.codescribe_root / "transcript-events.jsonl.generations.json"
        manifest.write_text(json.dumps({
            "schema": "codescribe.bus-generations.v1",
            "root": str(active_bus),
            "stream_id": "test-stream",
            "stream_inode": 100,
            "stream_dev": 1,
            "stream_birthtime": None,
            "segments": [
                {
                    "id": "seg-1",
                    "path": str(seg1),
                    "start": 0,
                    "length": len('{"event":"chunk1"}\n'),
                    "dev": 1,
                    "ino": 101,
                    "day": "2026-09-01",
                    "compressed": False,
                    "sha256": "fake",
                    "superseded": False,
                }
            ],
            "active": {
                "path": str(active_bus),
                "start": len('{"event":"chunk1"}\n'),
                "dev": 1,
                "ino": 102,
            },
            "pending": None,
        }))
        self.set_age(manifest, 45)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        self.assertNotIn(seg1.resolve(), candidate_paths)
        self.assertNotIn(active_bus.resolve(), candidate_paths)
        self.assertNotIn(manifest.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)
        self.assertTrue(seg1.exists())
        self.assertTrue(active_bus.exists())
        self.assertTrue(manifest.exists())

    def test_active_lease_and_followers_are_protected(self) -> None:
        """Active lease and its associated follower event/cursor files must not be pruned."""
        bridge = self.codescribe_root / "agent-bridge"
        leases_dir = bridge / "leases"
        followers_dir = bridge / "runtime" / "followers"
        leases_dir.mkdir(parents=True)
        followers_dir.mkdir(parents=True)

        # Active lease with current PID and fresh heartbeat
        active_lease_file = leases_dir / "codex-session-active.json"
        active_lease_file.write_text(json.dumps({
            "schema": "codescribe.agent-turn-lease.v1",
            "lease_id": "codex-session-active",
            "pid": os.getpid(),
            "heartbeat_unix": time.time(),
            "active": True,
        }))

        # Follower files for active lease (even if mtime is old)
        follower_events = followers_dir / "codex-session-active.events.jsonl"
        follower_events.write_text('{"seq":1}\n')
        self.set_age(follower_events, 20)

        follower_cursor = followers_dir / "codex-session-active.cursor.json"
        follower_cursor.write_text('{"cursor":10}\n')
        self.set_age(follower_cursor, 20)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        self.assertNotIn(active_lease_file.resolve(), candidate_paths)
        self.assertNotIn(follower_events.resolve(), candidate_paths)
        self.assertNotIn(follower_cursor.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)
        self.assertTrue(active_lease_file.exists())
        self.assertTrue(follower_events.exists())
        self.assertTrue(follower_cursor.exists())

    def test_dead_follower_logs_and_backup_dirs_moved_to_trash(self) -> None:
        """Dead follower logs (>24h, no active lease) and backup dirs are moved preserving paths."""
        bridge = self.codescribe_root / "agent-bridge"
        followers_dir = bridge / "runtime" / "followers"
        followers_dir.mkdir(parents=True)

        # 1. Dead follower log (>24h old, no lease in leases/)
        dead_events = followers_dir / "dead-lease-123.events.jsonl"
        dead_events.write_text('{"seq":99}\n')
        self.set_age(dead_events, 3)

        dead_cursor = followers_dir / "dead-lease-123.cursor.json"
        dead_cursor.write_text('{"cursor":5}\n')
        self.set_age(dead_cursor, 3)

        # 2. Old backup directories
        parked = bridge / "leases-parked-2026-10-10"
        parked.mkdir(parents=True)
        (parked / "old-lease.json").write_text("{}")

        bak = bridge / "runtime.bak-20260828-180721"
        bak.mkdir(parents=True)
        (bak / "old-runtime.json").write_text("{}")

        oversized = followers_dir / "codex.oversized-rows-20261010"
        oversized.mkdir(parents=True)
        (oversized / "row-1.jsonl").write_text("big\n")

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        categories = {c.category for c in plan.candidates}
        self.assertIn("dead_follower_logs", categories)
        self.assertIn("backup_dirs", categories)

        prune_mod.execute_prune_plan(plan)

        # Must no longer exist in codescribe_root
        self.assertFalse(dead_events.exists())
        self.assertFalse(dead_cursor.exists())
        self.assertFalse(parked.exists())
        self.assertFalse(bak.exists())
        self.assertFalse(oversized.exists())

        # Must exist in target_trash_dir preserving relative structure
        trash_dest = plan.target_trash_dir
        self.assertTrue((trash_dest / "agent-bridge/runtime/followers/dead-lease-123.events.jsonl").exists())
        self.assertTrue((trash_dest / "agent-bridge/runtime/followers/dead-lease-123.cursor.json").exists())
        self.assertTrue((trash_dest / "agent-bridge/leases-parked-2026-10-10/old-lease.json").exists())
        self.assertTrue((trash_dest / "agent-bridge/runtime.bak-20260828-180721/old-runtime.json").exists())
        self.assertTrue((trash_dest / "agent-bridge/runtime/followers/codex.oversized-rows-20261010/row-1.jsonl").exists())

    def test_expired_acknowledgments_and_sessions_moved_to_trash(self) -> None:
        """Expired sessions and acknowledgments are moved; fresh and golden files stay."""
        bridge = self.codescribe_root / "agent-bridge"
        acks_dir = bridge / "acknowledgments"
        sessions_dir = self.codescribe_root / "sessions"
        golden_dir = sessions_dir / "golden"

        acks_dir.mkdir(parents=True)
        sessions_dir.mkdir(parents=True)
        golden_dir.mkdir(parents=True)

        # Old acknowledgment (10 days old) -> prune
        old_ack = acks_dir / "ack-old.json"
        old_ack.write_text('{"ack":1}\n')
        self.set_age(old_ack, 10)

        # Fresh acknowledgment (2 days old) -> keep
        fresh_ack = acks_dir / "ack-fresh.json"
        fresh_ack.write_text('{"ack":2}\n')
        self.set_age(fresh_ack, 2)

        # Old session (10 days old) -> prune
        old_session = sessions_dir / "session-old.wav"
        old_session.write_text("WAV_DATA_OLD")
        self.set_age(old_session, 10)

        # Fresh session (2 days old) -> keep
        fresh_session = sessions_dir / "session-fresh.wav"
        fresh_session.write_text("WAV_DATA_FRESH")
        self.set_age(fresh_session, 2)

        # Golden session (40 days old) -> keep because in 'golden' directory
        golden_session = golden_dir / "calibration.wav"
        golden_session.write_text("WAV_CALIBRATION")
        self.set_age(golden_session, 40)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        self.assertIn(old_ack.resolve(), candidate_paths)
        self.assertIn(old_session.resolve(), candidate_paths)
        self.assertNotIn(fresh_ack.resolve(), candidate_paths)
        self.assertNotIn(fresh_session.resolve(), candidate_paths)
        self.assertNotIn(golden_session.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)

        # Moved
        self.assertFalse(old_ack.exists())
        self.assertFalse(old_session.exists())
        self.assertTrue((plan.target_trash_dir / "agent-bridge/acknowledgments/ack-old.json").exists())
        self.assertTrue((plan.target_trash_dir / "sessions/session-old.wav").exists())

        # Preserved
        self.assertTrue(fresh_ack.exists())
        self.assertTrue(fresh_session.exists())
        self.assertTrue(golden_session.exists())

    def test_dry_run_mode_does_not_move_files(self) -> None:
        """Dry-run should report planned candidates without moving anything."""
        sessions_dir = self.codescribe_root / "sessions"
        sessions_dir.mkdir(parents=True)
        old_file = sessions_dir / "old.wav"
        old_file.write_text("DATA")
        self.set_age(old_file, 15)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        self.assertEqual(plan.total_items, 1)
        # In dry run, execute_prune_plan is NOT called
        self.assertTrue(old_file.exists())
        self.assertFalse(plan.target_trash_dir.exists())

    def test_cli_execution_with_json_report(self) -> None:
        """CLI invocation with --json outputs valid JSON matching plan."""
        sessions_dir = self.codescribe_root / "sessions"
        sessions_dir.mkdir(parents=True)
        old_file = sessions_dir / "stale.wav"
        old_file.write_text("WAV_BYTES_FOR_JSON")
        self.set_age(old_file, 12)

        cmd = [
            sys.executable,
            str(MODULE_PATH),
            "--codescribe-root", str(self.codescribe_root),
            "--trash-root", str(self.trash_root),
            "--days", "7",
            "--apply",
            "--json",
        ]

        proc = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, f"CLI stderr: {proc.stderr}")

        data = json.loads(proc.stdout)
        self.assertTrue(data["apply"])
        self.assertEqual(data["total_items"], 1)
        self.assertIn("expired_sessions_and_transcriptions", data["categories"])
        self.assertFalse(old_file.exists())
        target_dir = Path(data["target_trash_dir"])
        self.assertTrue((target_dir / "sessions/stale.wav").exists())


if __name__ == "__main__":
    unittest.main()
