#!/usr/bin/env python3
"""Safe runtime-to-trash hygiene tool for Codescribe.

Moves historical backup folders, orphaned follower logs, expired bridge artifacts,
and old sessions/transcriptions from ~/.codescribe into a timestamped subfolder
in ~/.Trash/codescribe-cleanup-<YYYYMMDD-HHMMSS>/ while preserving directory structure.

Hard Invariants:
1. NEVER touches active .jsonl segments or active files referenced by any *.generations.json.
2. NEVER touches active leases or files associated with an active lease_id (fresh heartbeat or live PID).
3. NEVER deletes (rm/unlink) user or runtime data directly — always moves to Trash via shutil.move.
4. NEVER touches golden/pinned datasets or active last_session symlinks.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import datetime
import json
import os
from pathlib import Path
import shutil
import sys
import time
from typing import Any


SAFE_PRESERVED_DIR_NAMES = frozenset({
    "pinned",
    "golden",
    "goldens",
    "corpus",
    "assets",
    "data_assets",
    "models",
    "prompts",
    "settings",
    "bin",
})

SAFE_PRESERVED_TRANSCRIPTION_DATES = frozenset({
    "2026-08-13",
    "2026-07-25",
    "2026-04-21",
    "2026-02-11",
    "2026-01-17",
})


def process_is_alive(pid: Any) -> bool:
    """Return True if the given PID corresponds to a running process."""
    if not isinstance(pid, int) or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def get_path_size(path: Path) -> int:
    """Calculate total size in bytes for a file or directory tree."""
    if not path.exists():
        return 0
    if path.is_file() or path.is_symlink():
        try:
            return path.stat().st_size
        except OSError:
            return 0
    total = 0
    try:
        for root, _, files in os.walk(path):
            for f in files:
                fp = Path(root) / f
                try:
                    total += fp.stat().st_size
                except OSError:
                    continue
    except OSError:
        pass
    return total


@dataclass
class PruneCandidate:
    path: Path
    category: str
    is_dir: bool
    bytes: int
    mtime: float


@dataclass
class PrunePlan:
    codescribe_root: Path
    trash_root: Path
    target_trash_dir: Path
    days_threshold: int
    timestamp: str
    protected_paths_count: int
    active_leases_count: int
    candidates: list[PruneCandidate] = field(default_factory=list)

    @property
    def total_bytes(self) -> int:
        return sum(c.bytes for c in self.candidates)

    @property
    def total_items(self) -> int:
        return len(self.candidates)

    def to_dict(self, apply: bool) -> dict[str, Any]:
        categories: dict[str, dict[str, Any]] = {}
        for c in self.candidates:
            cat = categories.setdefault(c.category, {"count": 0, "bytes": 0, "items": []})
            cat["count"] += 1
            cat["bytes"] += c.bytes
            cat["items"].append({
                "path": str(c.path),
                "relative": str(c.path.relative_to(self.codescribe_root)),
                "is_dir": c.is_dir,
                "bytes": c.bytes,
                "mtime_iso": datetime.datetime.fromtimestamp(c.mtime, datetime.timezone.utc).isoformat(),
            })

        return {
            "codescribe_root": str(self.codescribe_root),
            "trash_root": str(self.trash_root),
            "target_trash_dir": str(self.target_trash_dir),
            "apply": apply,
            "days_threshold": self.days_threshold,
            "timestamp": self.timestamp,
            "protected_paths_count": self.protected_paths_count,
            "active_leases_count": self.active_leases_count,
            "categories": categories,
            "total_items": self.total_items,
            "total_bytes": self.total_bytes,
        }


def collect_generation_protected_paths(codescribe_root: Path) -> set[Path]:
    """Find all *.generations.json files and collect all referenced segment/active paths."""
    protected: set[Path] = set()

    for manifest_path in codescribe_root.rglob("*.generations.json"):
        protected.add(manifest_path.resolve())

        # Also protect the un-suffixed journal file if it exists beside it
        base_name = manifest_path.name[:-len(".generations.json")]
        companion = manifest_path.with_name(base_name)
        if companion.exists():
            protected.add(companion.resolve())
        companion_lock = manifest_path.with_name(f"{base_name}.generation.lock")
        if companion_lock.exists():
            protected.add(companion_lock.resolve())

        try:
            with manifest_path.open("r", encoding="utf-8") as f:
                data = json.load(f)
        except Exception:
            continue

        if not isinstance(data, dict):
            continue

        # Protect root path
        root_path = data.get("root")
        if isinstance(root_path, str):
            protected.add(Path(root_path).resolve())

        # Protect segments
        segments = data.get("segments")
        if isinstance(segments, list):
            for seg in segments:
                if isinstance(seg, dict):
                    sp = seg.get("path")
                    if isinstance(sp, str):
                        protected.add(Path(sp).resolve())

        # Protect active
        active = data.get("active")
        if isinstance(active, dict):
            ap = active.get("path")
            if isinstance(ap, str):
                protected.add(Path(ap).resolve())

        # Protect pending
        pending = data.get("pending")
        if isinstance(pending, dict):
            pp = pending.get("path")
            if isinstance(pp, str):
                protected.add(Path(pp).resolve())
            for sub_key in ("next", "closed"):
                sub = pending.get(sub_key)
                if isinstance(sub, dict):
                    sub_p = sub.get("path")
                    if isinstance(sub_p, str):
                        protected.add(Path(sub_p).resolve())

    return protected


def collect_active_leases(codescribe_root: Path, now: float) -> tuple[set[Path], set[str]]:
    """Scan leases and return (protected_lease_paths, active_lease_ids)."""
    protected: set[Path] = set()
    active_ids: set[str] = set()

    leases_dir = codescribe_root / "agent-bridge" / "leases"
    if not leases_dir.is_dir():
        return protected, active_ids

    for lease_file in leases_dir.glob("*.json"):
        lease_path = lease_file.resolve()
        lease_id = lease_file.stem
        is_active = False

        try:
            with lease_file.open("r", encoding="utf-8") as f:
                data = json.load(f)
        except Exception:
            data = None

        if isinstance(data, dict):
            declared_id = data.get("lease_id")
            if isinstance(declared_id, str) and declared_id:
                lease_id = declared_id

            pid = data.get("pid")
            heartbeat = data.get("heartbeat_unix")
            fresh_heartbeat = (
                isinstance(heartbeat, (int, float))
                and 0 <= (now - float(heartbeat)) <= 86400
            )

            try:
                mtime_fresh = (now - lease_file.stat().st_mtime) <= 86400
            except OSError:
                mtime_fresh = False

            if process_is_alive(pid) or fresh_heartbeat or (data.get("active") is True and mtime_fresh):
                is_active = True
        else:
            # If unparseable but recent, fail closed
            try:
                if (now - lease_file.stat().st_mtime) <= 86400:
                    is_active = True
            except OSError:
                pass

        if is_active:
            protected.add(lease_path)
            lock_path = lease_file.with_suffix(".lock").resolve()
            if lock_path.exists():
                protected.add(lock_path)
            active_ids.add(lease_id)

    return protected, active_ids


def is_path_safe_from_prune(path: Path, protected_paths: set[Path]) -> bool:
    """Check if the path or any of its parents is protected or in a golden/pinned dir."""
    resolved = path.resolve()
    if resolved in protected_paths:
        return True

    # Check reserved directory components
    for part in path.parts:
        if part in SAFE_PRESERVED_DIR_NAMES:
            return True

    return False


def build_prune_plan(
    codescribe_root: Path,
    trash_root: Path,
    days: int = 7,
    now: float | None = None,
) -> PrunePlan:
    """Build a deterministic plan of files and folders to move into trash."""
    codescribe_root = codescribe_root.resolve()
    trash_root = trash_root.resolve()
    if now is None:
        now = time.time()

    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d-%H%M%S")
    target_trash_dir = trash_root / f"codescribe-cleanup-{timestamp}"

    protected_paths = collect_generation_protected_paths(codescribe_root)
    protected_leases, active_lease_ids = collect_active_leases(codescribe_root, now)
    protected_paths.update(protected_leases)

    # Protect active symlinks in codescribe_root (e.g. last_session.wav)
    for entry in codescribe_root.iterdir():
        if entry.is_symlink():
            protected_paths.add(entry.resolve())
            try:
                target = entry.resolve(strict=False)
                protected_paths.add(target)
            except OSError:
                pass

    candidates: list[PruneCandidate] = []
    seen_paths: set[Path] = set()

    # 1. Backup directories in agent-bridge
    agent_bridge = codescribe_root / "agent-bridge"
    if agent_bridge.is_dir():
        for item in agent_bridge.iterdir():
            if item.is_dir() and (item.name.startswith("leases-parked-") or item.name.startswith("runtime.bak-")):
                if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                    seen_paths.add(item)
                    candidates.append(PruneCandidate(
                        path=item,
                        category="backup_dirs",
                        is_dir=True,
                        bytes=get_path_size(item),
                        mtime=item.stat().st_mtime,
                    ))

        # Check runtime/followers/*.oversized-rows-*
        followers_dir = agent_bridge / "runtime" / "followers"
        if followers_dir.is_dir():
            for item in followers_dir.iterdir():
                if item.is_dir() and ".oversized-rows-" in item.name:
                    if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                        seen_paths.add(item)
                        candidates.append(PruneCandidate(
                            path=item,
                            category="backup_dirs",
                            is_dir=True,
                            bytes=get_path_size(item),
                            mtime=item.stat().st_mtime,
                        ))

            # 2. Dead follower logs in agent-bridge/runtime/followers/<lease>.*
            for item in followers_dir.iterdir():
                if not item.is_file():
                    continue

                # Match patterns: <lease_id>.events.jsonl, <lease_id>.cursor.json, <lease_id>.log, etc.
                name = item.name
                lease_id = ""
                for suffix in (".events.jsonl", ".cursor.json", ".log", ".pid", ".errors.log"):
                    if name.endswith(suffix):
                        lease_id = name[:-len(suffix)]
                        break

                if not lease_id or lease_id in active_lease_ids:
                    continue

                mtime = item.stat().st_mtime
                if (now - mtime) > 86400:  # Older than 24h
                    if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                        seen_paths.add(item)
                        candidates.append(PruneCandidate(
                            path=item,
                            category="dead_follower_logs",
                            is_dir=False,
                            bytes=get_path_size(item),
                            mtime=mtime,
                        ))

        # 3. Expired bridge artifacts (acknowledgments, wakeups, receipts, reply-sources, reply-playback, speech)
        expiry_seconds = days * 86400
        bridge_dirs_to_check = [
            agent_bridge / "acknowledgments",
            agent_bridge / "wakeups",
            agent_bridge / "runtime" / "receipts",
            agent_bridge / "runtime" / "reply-sources",
            agent_bridge / "runtime" / "reply-playback",
        ]

        for bdir in bridge_dirs_to_check:
            if not bdir.is_dir():
                continue
            for item in bdir.iterdir():
                if not item.is_file() and not item.is_dir():
                    continue

                # Do not delete subdirectories that match active leases
                if any(active_id in item.name for active_id in active_lease_ids):
                    continue

                mtime = item.stat().st_mtime
                if (now - mtime) > expiry_seconds:
                    if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                        seen_paths.add(item)
                        candidates.append(PruneCandidate(
                            path=item,
                            category="expired_bridge_files",
                            is_dir=item.is_dir(),
                            bytes=get_path_size(item),
                            mtime=mtime,
                        ))

        # Check agent-bridge/speech/*.wav
        speech_dir = agent_bridge / "speech"
        if speech_dir.is_dir():
            for item in speech_dir.iterdir():
                if not item.is_file() or not item.name.endswith(".wav"):
                    continue
                try:
                    mtime = item.stat().st_mtime
                except OSError:
                    continue
                if (now - mtime) > expiry_seconds:
                    if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                        seen_paths.add(item)
                        candidates.append(PruneCandidate(
                            path=item,
                            category="expired_bridge_files",
                            is_dir=False,
                            bytes=get_path_size(item),
                            mtime=mtime,
                        ))

    expiry_seconds = days * 86400

    # 4. Expired sessions in codescribe_root/sessions/ (run BEFORE takes/transcriptions
    # so retained .audio-retention.json receipts protect their owned takes/transcriptions).
    sessions_dir = codescribe_root / "sessions"
    if sessions_dir.is_dir():
        for root, dirs, files in os.walk(sessions_dir):
            dirs[:] = [d for d in dirs if d not in SAFE_PRESERVED_DIR_NAMES]
            root_path = Path(root)

            session_groups: dict[str, list[Path]] = {}
            for fname in files:
                fpath = root_path / fname
                session_id = fname.split(".", 1)[0]
                session_groups.setdefault(session_id, []).append(fpath)

            for session_id, group_files in session_groups.items():
                receipt_owned_paths: list[Path] = []
                for fp in group_files:
                    if fp.name.endswith(".audio-retention.json"):
                        try:
                            with fp.open("r", encoding="utf-8") as rf:
                                rdata = json.load(rf)
                            if isinstance(rdata, dict) and isinstance(rdata.get("files"), list):
                                for entry in rdata["files"]:
                                    if isinstance(entry, dict) and isinstance(entry.get("relative"), str):
                                        owned = codescribe_root / entry["relative"]
                                        if owned.exists():
                                            receipt_owned_paths.append(owned)
                        except Exception:
                            pass

                group_protected = any(
                    is_path_safe_from_prune(fp, protected_paths) for fp in group_files
                ) or any(
                    is_path_safe_from_prune(op, protected_paths)
                    or any(part in SAFE_PRESERVED_TRANSCRIPTION_DATES for part in op.parts)
                    for op in receipt_owned_paths
                )

                all_expired = not group_protected
                if all_expired:
                    for fp in group_files + receipt_owned_paths:
                        try:
                            mtime = fp.stat().st_mtime
                        except OSError:
                            all_expired = False
                            break
                        if (now - mtime) <= expiry_seconds:
                            all_expired = False
                            break

                if all_expired:
                    for fp in group_files:
                        if fp not in seen_paths:
                            seen_paths.add(fp)
                            candidates.append(PruneCandidate(
                                path=fp,
                                category="expired_sessions_and_transcriptions",
                                is_dir=False,
                                bytes=get_path_size(fp),
                                mtime=fp.stat().st_mtime,
                            ))
                else:
                    for op in receipt_owned_paths:
                        protected_paths.add(op.resolve())

    # 5. Expired takes in codescribe_root/takes/*.wav
    takes_dir = codescribe_root / "takes"
    if takes_dir.is_dir():
        for root, dirs, files in os.walk(takes_dir):
            dirs[:] = [d for d in dirs if d not in SAFE_PRESERVED_DIR_NAMES]
            root_path = Path(root)
            for fname in files:
                if not fname.endswith(".wav"):
                    continue
                fpath = root_path / fname
                try:
                    mtime = fpath.stat().st_mtime
                except OSError:
                    continue
                if (now - mtime) > expiry_seconds:
                    if fpath not in seen_paths and not is_path_safe_from_prune(fpath, protected_paths):
                        seen_paths.add(fpath)
                        candidates.append(PruneCandidate(
                            path=fpath,
                            category="expired_takes",
                            is_dir=False,
                            bytes=get_path_size(fpath),
                            mtime=mtime,
                        ))

    # 6. Expired replays in codescribe_root/replays/replay-*
    replays_dir = codescribe_root / "replays"
    if replays_dir.is_dir():
        for item in replays_dir.iterdir():
            if not item.name.startswith("replay-"):
                continue
            try:
                mtime = item.stat().st_mtime
            except OSError:
                continue
            if (now - mtime) > expiry_seconds:
                if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                    seen_paths.add(item)
                    candidates.append(PruneCandidate(
                        path=item,
                        category="expired_replays",
                        is_dir=item.is_dir(),
                        bytes=get_path_size(item),
                        mtime=mtime,
                    ))

    # 7. Rotated logs in codescribe_root/data.noindex/logs/*.1
    logs_dir = codescribe_root / "data.noindex" / "logs"
    if logs_dir.is_dir():
        for item in logs_dir.iterdir():
            if not item.is_file():
                continue
            # NEVER touch active logs, only rotated logs ending with .1
            if not item.name.endswith(".1"):
                continue
            try:
                mtime = item.stat().st_mtime
            except OSError:
                continue
            if (now - mtime) > expiry_seconds:
                if item not in seen_paths and not is_path_safe_from_prune(item, protected_paths):
                    seen_paths.add(item)
                    candidates.append(PruneCandidate(
                        path=item,
                        category="rotated_logs",
                        is_dir=False,
                        bytes=get_path_size(item),
                        mtime=mtime,
                    ))

    # 8. Expired transcriptions in codescribe_root/transcriptions/
    # Only prune expired audio files (.m4a, .wav). NEVER prune text files (.txt, .md)
    # or truth sidecars (.revisions.jsonl, .truth.json). Skip benchmark date subdirectories.
    transcriptions_dir = codescribe_root / "transcriptions"
    if transcriptions_dir.is_dir():
        for root, dirs, files in os.walk(transcriptions_dir):
            dirs[:] = [
                d for d in dirs
                if d not in SAFE_PRESERVED_DIR_NAMES and d not in SAFE_PRESERVED_TRANSCRIPTION_DATES
            ]
            root_path = Path(root)

            if any(part in SAFE_PRESERVED_TRANSCRIPTION_DATES for part in root_path.parts):
                continue

            for fname in files:
                if not (fname.endswith(".m4a") or fname.endswith(".wav")):
                    continue

                fpath = root_path / fname
                try:
                    mtime = fpath.stat().st_mtime
                except OSError:
                    continue

                if (now - mtime) > expiry_seconds:
                    if fpath not in seen_paths and not is_path_safe_from_prune(fpath, protected_paths):
                        seen_paths.add(fpath)
                        candidates.append(PruneCandidate(
                            path=fpath,
                            category="expired_sessions_and_transcriptions",
                            is_dir=False,
                            bytes=get_path_size(fpath),
                            mtime=mtime,
                        ))

    return PrunePlan(
        codescribe_root=codescribe_root,
        trash_root=trash_root,
        target_trash_dir=target_trash_dir,
        days_threshold=days,
        timestamp=timestamp,
        protected_paths_count=len(protected_paths),
        active_leases_count=len(active_lease_ids),
        candidates=candidates,
    )


def execute_prune_plan(plan: PrunePlan) -> None:
    """Move all planned candidates into the timestamped trash directory."""
    if not plan.candidates:
        return

    plan.target_trash_dir.mkdir(parents=True, exist_ok=True)

    for candidate in plan.candidates:
        if not candidate.path.exists():
            continue
        rel = candidate.path.relative_to(plan.codescribe_root)
        dest = plan.target_trash_dir / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(candidate.path), str(dest))


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Safe runtime hygiene: moves expired Codescribe artifacts to ~/.Trash."
    )
    parser.add_argument(
        "--codescribe-root",
        type=Path,
        default=Path.home() / ".codescribe",
        help="Root Codescribe runtime directory (default: ~/.codescribe)",
    )
    parser.add_argument(
        "--trash-root",
        type=Path,
        default=Path.home() / ".Trash",
        help="Root Trash directory (default: ~/.Trash)",
    )
    parser.add_argument(
        "--days",
        type=int,
        default=7,
        help="Retention threshold in days for sessions, transcriptions, and acknowledgments (default: 7)",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Execute move to Trash. If not supplied, runs in dry-run mode.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output JSON summary report.",
    )

    args = parser.parse_args()

    codescribe_root = args.codescribe_root.expanduser().resolve()
    trash_root = args.trash_root.expanduser().resolve()

    if not codescribe_root.exists():
        sys.stderr.write(f"Error: Codescribe root does not exist: {codescribe_root}\n")
        sys.exit(1)

    plan = build_prune_plan(
        codescribe_root=codescribe_root,
        trash_root=trash_root,
        days=args.days,
    )

    if args.apply:
        execute_prune_plan(plan)

    report = plan.to_dict(apply=args.apply)

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        status_label = "APPLIED (Moved to Trash)" if args.apply else "DRY-RUN (No files moved)"
        print(f"=== Codescribe Runtime Hygiene [{status_label}] ===")
        print(f"Source Root:        {plan.codescribe_root}")
        print(f"Destination Trash:  {plan.target_trash_dir}")
        print(f"Days Threshold:     {plan.days_threshold} days")
        print(f"Protected Paths:    {plan.protected_paths_count} items")
        print(f"Active Leases:      {plan.active_leases_count} leases")
        print("--------------------------------------------------")
        for cat_name, cat_data in report["categories"].items():
            mb = cat_data["bytes"] / (1024 * 1024)
            print(f"  * {cat_name:35s}: {cat_data['count']:5d} items ({mb:8.2f} MB)")
        total_mb = plan.total_bytes / (1024 * 1024)
        print("--------------------------------------------------")
        print(f"Total: {plan.total_items} items ({total_mb:.2f} MB)")


if __name__ == "__main__":
    main()
