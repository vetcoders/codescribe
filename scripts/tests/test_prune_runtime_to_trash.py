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

    def test_transcription_texts_and_dates_are_protected(self) -> None:
        """In transcriptions/, only audio (.wav/.m4a) is moved; texts, sidecars and safe dates stay."""
        trans_dir = self.codescribe_root / "transcriptions"
        golden_date_dir = trans_dir / "2026-08-13"
        normal_date_dir = trans_dir / "2026-10-01"

        golden_date_dir.mkdir(parents=True)
        normal_date_dir.mkdir(parents=True)

        # Audio in safe benchmark date directory (10 days old) -> MUST BE PRESERVED
        golden_audio = golden_date_dir / "010203_golden.wav"
        golden_audio.write_text("AUDIO_GOLDEN")
        self.set_age(golden_audio, 10)

        # Audio in normal date directory (10 days old) -> MUST BE PRUNED
        normal_wav = normal_date_dir / "010203_normal.wav"
        normal_wav.write_text("AUDIO_WAV")
        self.set_age(normal_wav, 10)

        normal_m4a = normal_date_dir / "010203_normal.m4a"
        normal_m4a.write_text("AUDIO_M4A")
        self.set_age(normal_m4a, 10)

        # Non-audio files in normal date directory (10 days old) -> MUST ALL BE PRESERVED
        txt_file = normal_date_dir / "010203_normal.txt"
        txt_file.write_text("TRANSCRIPT TEXT")
        self.set_age(txt_file, 10)

        md_file = normal_date_dir / "010203_normal.md"
        md_file.write_text("# Markdown Transcript")
        self.set_age(md_file, 10)

        truth_file = normal_date_dir / "010203_normal.truth.json"
        truth_file.write_text('{"truth": true}')
        self.set_age(truth_file, 10)

        rev_file = normal_date_dir / "010203_normal.revisions.jsonl"
        rev_file.write_text('{"rev": 1}\n')
        self.set_age(rev_file, 10)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        self.assertIn(normal_wav.resolve(), candidate_paths)
        self.assertIn(normal_m4a.resolve(), candidate_paths)
        self.assertNotIn(golden_audio.resolve(), candidate_paths)
        self.assertNotIn(txt_file.resolve(), candidate_paths)
        self.assertNotIn(md_file.resolve(), candidate_paths)
        self.assertNotIn(truth_file.resolve(), candidate_paths)
        self.assertNotIn(rev_file.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)

        # Audio moved
        self.assertFalse(normal_wav.exists())
        self.assertFalse(normal_m4a.exists())
        self.assertTrue((plan.target_trash_dir / "transcriptions/2026-10-01/010203_normal.wav").exists())
        self.assertTrue((plan.target_trash_dir / "transcriptions/2026-10-01/010203_normal.m4a").exists())

        # Texts, sidecars, and golden date audio preserved
        self.assertTrue(golden_audio.exists())
        self.assertTrue(txt_file.exists())
        self.assertTrue(md_file.exists())
        self.assertTrue(truth_file.exists())
        self.assertTrue(rev_file.exists())

    def test_session_group_consistency(self) -> None:
        """Sessions are pruned only if ALL files sharing the session_id prefix are expired."""
        sessions_dir = self.codescribe_root / "sessions"
        sessions_dir.mkdir(parents=True)

        # Session 1: audio is old (10 days), but audio-retention is fresh (2 days) -> KEEP ALL
        s1_wav = sessions_dir / "session-1.wav"
        s1_wav.write_text("WAV_1")
        self.set_age(s1_wav, 10)

        s1_ret = sessions_dir / "session-1.audio-retention.json"
        s1_ret.write_text('{"status": "active"}')
        self.set_age(s1_ret, 2)

        # Session 2: all files are old (10 days) -> PRUNE ALL
        s2_wav = sessions_dir / "session-2.wav"
        s2_wav.write_text("WAV_2")
        self.set_age(s2_wav, 10)

        s2_json = sessions_dir / "session-2.json"
        s2_json.write_text('{"id": "session-2"}')
        self.set_age(s2_json, 10)

        s2_slots = sessions_dir / "session-2.slots.jsonl"
        s2_slots.write_text('{"slot": 1}\n')
        self.set_age(s2_slots, 10)

        s2_ret = sessions_dir / "session-2.audio-retention.json"
        s2_ret.write_text('{"status": "expired"}')
        self.set_age(s2_ret, 10)

        # Session 3: all files old, but s3.wav is linked by last_session.wav -> KEEP ALL
        s3_wav = sessions_dir / "session-3.wav"
        s3_wav.write_text("WAV_3")
        self.set_age(s3_wav, 10)

        s3_ret = sessions_dir / "session-3.audio-retention.json"
        s3_ret.write_text('{"status": "linked"}')
        self.set_age(s3_ret, 10)

        symlink = self.codescribe_root / "last_session.wav"
        symlink.symlink_to(Path("sessions") / "session-3.wav")

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        # Session 1: preserved because of fresh retention file
        self.assertNotIn(s1_wav.resolve(), candidate_paths)
        self.assertNotIn(s1_ret.resolve(), candidate_paths)

        # Session 2: all files expired -> moved
        self.assertIn(s2_wav.resolve(), candidate_paths)
        self.assertIn(s2_json.resolve(), candidate_paths)
        self.assertIn(s2_slots.resolve(), candidate_paths)
        self.assertIn(s2_ret.resolve(), candidate_paths)

        # Session 3: preserved because s3_wav is target of last_session.wav symlink
        self.assertNotIn(s3_wav.resolve(), candidate_paths)
        self.assertNotIn(s3_ret.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)

        self.assertTrue(s1_wav.exists())
        self.assertTrue(s1_ret.exists())
        self.assertFalse(s2_wav.exists())
        self.assertFalse(s2_json.exists())
        self.assertFalse(s2_slots.exists())
        self.assertFalse(s2_ret.exists())
        self.assertTrue(s3_wav.exists())
        self.assertTrue(s3_ret.exists())

        trash_dest = plan.target_trash_dir
        self.assertTrue((trash_dest / "sessions/session-2.wav").exists())
        self.assertTrue((trash_dest / "sessions/session-2.json").exists())
        self.assertTrue((trash_dest / "sessions/session-2.slots.jsonl").exists())
        self.assertTrue((trash_dest / "sessions/session-2.audio-retention.json").exists())

    def test_additional_bloat_categories(self) -> None:
        """Verifies takes, replays, agent-bridge/speech, and data.noindex/logs categories."""
        # 1. takes/*.wav
        takes_dir = self.codescribe_root / "takes"
        takes_dir.mkdir(parents=True)

        old_take = takes_dir / "codescribe_recording_1700000000.wav"
        old_take.write_text("TAKE_PCM")
        self.set_age(old_take, 10)

        fresh_take = takes_dir / "codescribe_recording_1800000000.wav"
        fresh_take.write_text("TAKE_PCM_FRESH")
        self.set_age(fresh_take, 2)

        txt_take = takes_dir / "notes.txt"
        txt_take.write_text("NOTE")
        self.set_age(txt_take, 10)

        # 2. replays/replay-*
        replays_dir = self.codescribe_root / "replays"
        replays_dir.mkdir(parents=True)

        old_replay = replays_dir / "replay-20260901-test"
        old_replay.mkdir()
        (old_replay / "data.raw").write_text("RAW")
        self.set_age(old_replay, 10)

        fresh_replay = replays_dir / "replay-fresh"
        fresh_replay.mkdir()
        self.set_age(fresh_replay, 2)

        other_replay_dir = replays_dir / "other-dir"
        other_replay_dir.mkdir()
        self.set_age(other_replay_dir, 10)

        # 3. agent-bridge/speech/*.wav
        speech_dir = self.codescribe_root / "agent-bridge" / "speech"
        speech_dir.mkdir(parents=True)

        old_speech = speech_dir / "tts_old.wav"
        old_speech.write_text("TTS_OLD")
        self.set_age(old_speech, 10)

        fresh_speech = speech_dir / "tts_fresh.wav"
        fresh_speech.write_text("TTS_FRESH")
        self.set_age(fresh_speech, 2)

        # 4. data.noindex/logs/*.1
        logs_dir = self.codescribe_root / "data.noindex" / "logs"
        logs_dir.mkdir(parents=True)

        rot_log1 = logs_dir / "codescribe.log.1"
        rot_log1.write_text("ROTATED LOG 1")
        self.set_age(rot_log1, 10)

        rot_log2 = logs_dir / "diagnostic.log.1"
        rot_log2.write_text("ROTATED LOG 2")
        self.set_age(rot_log2, 10)

        active_log1 = logs_dir / "codescribe.log"
        active_log1.write_text("ACTIVE LOG 1")
        self.set_age(active_log1, 10)  # Even if old mtime, NEVER touch active log

        active_log2 = logs_dir / "diagnostic.log"
        active_log2.write_text("ACTIVE LOG 2")
        self.set_age(active_log2, 10)

        plan = prune_mod.build_prune_plan(
            codescribe_root=self.codescribe_root,
            trash_root=self.trash_root,
            days=7,
        )

        categories = {c.category for c in plan.candidates}
        self.assertIn("expired_takes", categories)
        self.assertIn("expired_replays", categories)
        self.assertIn("expired_bridge_files", categories)
        self.assertIn("rotated_logs", categories)

        candidate_paths = {c.path.resolve() for c in plan.candidates}
        self.assertIn(old_take.resolve(), candidate_paths)
        self.assertNotIn(fresh_take.resolve(), candidate_paths)
        self.assertNotIn(txt_take.resolve(), candidate_paths)

        self.assertIn(old_replay.resolve(), candidate_paths)
        self.assertNotIn(fresh_replay.resolve(), candidate_paths)
        self.assertNotIn(other_replay_dir.resolve(), candidate_paths)

        self.assertIn(old_speech.resolve(), candidate_paths)
        self.assertNotIn(fresh_speech.resolve(), candidate_paths)

        self.assertIn(rot_log1.resolve(), candidate_paths)
        self.assertIn(rot_log2.resolve(), candidate_paths)
        self.assertNotIn(active_log1.resolve(), candidate_paths)
        self.assertNotIn(active_log2.resolve(), candidate_paths)

        prune_mod.execute_prune_plan(plan)

        self.assertFalse(old_take.exists())
        self.assertTrue(fresh_take.exists())
        self.assertTrue(txt_take.exists())

        self.assertFalse(old_replay.exists())
        self.assertTrue(fresh_replay.exists())
        self.assertTrue(other_replay_dir.exists())

        self.assertFalse(old_speech.exists())
        self.assertTrue(fresh_speech.exists())

        self.assertFalse(rot_log1.exists())
        self.assertFalse(rot_log2.exists())
        self.assertTrue(active_log1.exists())
        self.assertTrue(active_log2.exists())


if __name__ == "__main__":
    unittest.main()
