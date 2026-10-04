#!/usr/bin/env python3
"""Repair the Codescribe session-audio archive without deleting a sample.

``~/.codescribe/sessions`` accumulates byte-identical takes (one source
retranscribed N times costs N copies) and QuickTime/MP4 files wearing a
``.wav`` name. This script turns duplicates into hardlinks to the oldest
copy and reports the shells. Dry-run is the default; ``--apply`` writes.

Hardlinks are safe here because of how the archive is written:
``app/controller/mod.rs:451`` ``publish_retained_audio`` and (after W1-2)
``app/presentation/cli_transcript_lane.rs`` write session audio as new
inodes and rename them into place; no code path truncates a
``sessions/*.wav`` in place, so names may share one inode. A duplicate name
is never removed directly: the canonical inode is linked to a temporary
name in the same directory and that name is ``os.replace``d over the
duplicate, so every name resolves to audio bytes at every instant.
``last_session.wav`` is never linked or renamed.

A session may be in flight: the run is refused (exit 3) when any regular
file in the directory changed within the last 60 seconds, unless
``--force``. Symlinks are never followed; they are skipped and reported.

Exit codes: 0 ok, 2 nothing to do, 3 refused (in-flight / not a
directory), 1 error.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

LAST_SESSION_WAV = "last_session.wav"
TMP_PREFIX = ".dedupe-"
HASH_BUFFER_BYTES = 8 * 1024 * 1024
IN_FLIGHT_SECONDS = 60.0
REPORT_SCHEMA = "codescribe.sessions-dedupe.v1"
RENAME_EXTENSIONS = {"mov": ".mov", "m4a": ".m4a", "mp4": ".mp4"}


@dataclass
class Entry:
    name: str
    path: Path
    size: int
    mtime: float
    ino: int
    nlink: int


@dataclass
class Duplicate:
    entry: Entry
    shared_inode: bool
    action: str


@dataclass
class Group:
    sha256: str
    size: int
    canonical: Entry
    members: list[Entry]
    duplicates: list[Duplicate]
    bytes_reclaimable: int


@dataclass
class Shell:
    name: str
    container: str
    brand: str | None
    action: str
    renamed_to: str | None


def default_sessions_dir(env: dict[str, str]) -> Path:
    if "CODESCRIBE_DATA_DIR" in env:
        raw = env["CODESCRIBE_DATA_DIR"]
        path = Path(os.path.expanduser(raw))
        if raw:
            try:
                return path.resolve(strict=True) / "sessions"
            except OSError:
                pass
        return path / "sessions"
    return Path.home() / ".codescribe" / "sessions"


def human_size(count: int) -> str:
    value = float(count)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            if unit == "B":
                return f"{int(value)} B"
            return f"{value:.1f} {unit}"
        value /= 1024.0
    return f"{count} B"


def classify_header(path: Path) -> tuple[str, str | None]:
    """Return (container, brand) from the first 12 bytes.

    RIFF WAV starts with ``RIFF????WAVE``; QuickTime/MP4 carries ``ftyp``
    at bytes 4-8 with the brand at bytes 8-12 (``qt  `` -> mov,
    ``M4A `` -> m4a, ``isom``/``mp42``/... -> mp4).
    """
    try:
        with path.open("rb") as handle:
            header = handle.read(12)
    except OSError:
        return "other", None
    if len(header) >= 12 and header[:4] == b"RIFF" and header[8:12] == b"WAVE":
        return "wav", None
    if len(header) >= 8 and header[4:8] == b"ftyp":
        brand = header[8:12]
        if brand == b"qt  ":
            return "mov", "qt  "
        if brand == b"M4A ":
            return "m4a", "M4A "
        text = brand.decode("ascii", errors="replace").strip()
        return "mp4", text or None
    return "other", None


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(HASH_BUFFER_BYTES)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def scan(
    target: Path,
) -> tuple[list[Entry], list[dict[str, str]], tuple[str, float] | None, int]:
    """List regular files without ever following symlinks.

    Returns (candidates, skipped, freshest, regular_files): candidates are
    eligible for grouping, skipped names carry a reason, freshest is the
    (name, mtime) of the most recently modified regular file of any kind,
    and regular_files counts every regular file seen.
    """
    candidates: list[Entry] = []
    skipped: list[dict[str, str]] = []
    freshest: tuple[str, float] | None = None
    regular_files = 0
    with os.scandir(target) as listing:
        for item in listing:
            if item.is_symlink():
                skipped.append({"name": item.name, "reason": "symlink"})
                continue
            if not item.is_file(follow_symlinks=False):
                continue
            stat = item.stat(follow_symlinks=False)
            regular_files += 1
            if freshest is None or stat.st_mtime > freshest[1]:
                freshest = (item.name, stat.st_mtime)
            if item.name == LAST_SESSION_WAV:
                skipped.append({"name": item.name, "reason": "protected"})
                continue
            if item.name.startswith(TMP_PREFIX):
                skipped.append({"name": item.name, "reason": "dedupe-temp-residue"})
                continue
            candidates.append(
                Entry(
                    name=item.name,
                    path=Path(item.path),
                    size=stat.st_size,
                    mtime=stat.st_mtime,
                    ino=stat.st_ino,
                    nlink=stat.st_nlink,
                )
            )
    candidates.sort(key=lambda entry: entry.name)
    skipped.sort(key=lambda row: row["name"])
    return candidates, skipped, freshest, regular_files


def find_groups(candidates: list[Entry], min_size: int) -> tuple[list[Group], list[str]]:
    """Group by size, then by SHA-256 within a size class."""
    by_size: dict[int, list[Entry]] = {}
    for entry in candidates:
        if entry.size < min_size:
            continue
        by_size.setdefault(entry.size, []).append(entry)

    groups: list[Group] = []
    errors: list[str] = []
    for size, members in sorted(by_size.items()):
        if len(members) < 2:
            continue
        by_digest: dict[str, list[Entry]] = {}
        for member in members:
            try:
                digest = sha256_of(member.path)
            except OSError as error:
                errors.append(f"{member.name}: {error}")
                continue
            by_digest.setdefault(digest, []).append(member)
        for digest, copies in sorted(by_digest.items()):
            if len(copies) < 2:
                continue
            canonical = min(copies, key=lambda entry: (entry.mtime, entry.name))
            duplicates: list[Duplicate] = []
            foreign_inodes: set[int] = set()
            for entry in copies:
                if entry is canonical:
                    continue
                shared = entry.ino == canonical.ino
                if not shared:
                    foreign_inodes.add(entry.ino)
                duplicates.append(
                    Duplicate(
                        entry=entry,
                        shared_inode=shared,
                        action="already-linked" if shared else "pending",
                    )
                )
            groups.append(
                Group(
                    sha256=digest,
                    size=size,
                    canonical=canonical,
                    members=sorted(copies, key=lambda entry: entry.name),
                    duplicates=duplicates,
                    # Byte accounting assumes no hardlinks outside this
                    # directory: each non-canonical inode frees its bytes
                    # once every name here points at the canonical inode.
                    bytes_reclaimable=size * len(foreign_inodes),
                )
            )
    groups.sort(key=lambda group: (-group.bytes_reclaimable, group.sha256))
    return groups, errors


def link_duplicate(canonical: Path, duplicate: Path, target: Path, counter: list[int]) -> None:
    """Point ``duplicate`` at ``canonical``'s inode, atomically per name."""
    while True:
        counter[0] += 1
        tmp = target / f"{TMP_PREFIX}{os.getpid()}-{counter[0]}.tmp"
        try:
            os.link(canonical, tmp)
            break
        except FileExistsError:
            continue
    try:
        os.replace(tmp, duplicate)
    except BaseException:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise


def find_shells(candidates: list[Entry]) -> list[Shell]:
    """A ``.wav`` name whose header is not RIFF is a shell."""
    shells: list[Shell] = []
    for entry in candidates:
        if not entry.name.endswith(".wav"):
            continue
        container, brand = classify_header(entry.path)
        if container == "wav":
            continue
        shells.append(
            Shell(
                name=entry.name,
                container=container,
                brand=brand,
                action="reported",
                renamed_to=None,
            )
        )
    return shells


def apply_groups(groups: list[Group], target: Path) -> None:
    counter = [0]
    for group in groups:
        for duplicate in group.duplicates:
            if duplicate.action != "pending":
                continue
            link_duplicate(group.canonical.path, duplicate.entry.path, target, counter)
            duplicate.action = "linked"


def apply_shell_renames(shells: list[Shell], target: Path) -> None:
    for shell in shells:
        extension = RENAME_EXTENSIONS.get(shell.container)
        if extension is None:
            shell.action = "rename-skipped-unknown-container"
            continue
        new_name = shell.name[: -len(".wav")] + extension
        if os.path.lexists(target / new_name):
            shell.action = "rename-skipped-target-exists"
            continue
        os.rename(target / shell.name, target / new_name)
        shell.action = "renamed"
        shell.renamed_to = new_name


def group_json(group: Group) -> dict[str, object]:
    return {
        "sha256": group.sha256,
        "size": group.size,
        "canonical": group.canonical.name,
        "members": [entry.name for entry in group.members],
        "duplicates": [
            {
                "name": duplicate.entry.name,
                "shared_inode": duplicate.shared_inode,
                "action": duplicate.action,
            }
            for duplicate in group.duplicates
        ],
        "bytes_reclaimable": group.bytes_reclaimable,
    }


def shell_json(shell: Shell) -> dict[str, object]:
    row: dict[str, object] = {
        "name": shell.name,
        "container": shell.container,
        "brand": shell.brand,
        "action": shell.action,
    }
    if shell.renamed_to is not None:
        row["renamed_to"] = shell.renamed_to
    return row


def human_summary(
    *,
    target: Path,
    mode: str,
    regular_files: int,
    candidates: int,
    symlinks: int,
    groups: list[Group],
    shells: list[Shell],
    skipped: list[dict[str, str]],
    bytes_reclaimable: int,
    bytes_reclaimed: int,
) -> str:
    lines = [
        f"sessions-dedupe: dir={target} mode={mode}",
        f"scanned: {regular_files} regular files, "
        f"{symlinks} symlink(s) skipped, {candidates} candidates",
    ]
    if groups:
        lines.append(
            f"duplicate groups: {len(groups)} "
            f"(reclaimable {human_size(bytes_reclaimable)}, "
            f"reclaimed {human_size(bytes_reclaimed)})"
        )
        for group in groups:
            lines.append(
                f"  {group.sha256[:12]}  "
                f"{len(group.members)} x {human_size(group.size)}  "
                f"canonical={group.canonical.name}  "
                f"reclaimable={human_size(group.bytes_reclaimable)}"
            )
            by_action: dict[str, list[str]] = {}
            for duplicate in group.duplicates:
                by_action.setdefault(duplicate.action, []).append(duplicate.entry.name)
            for action, names in sorted(by_action.items()):
                lines.append(f"    {action}: {', '.join(sorted(names))}")
    else:
        lines.append("duplicate groups: 0")
    if shells:
        lines.append(f"shells (.wav name, other container): {len(shells)}")
        for shell in shells:
            if shell.action == "renamed":
                lines.append(f"  {shell.name} -> {shell.renamed_to} ({shell.container})")
            else:
                brand = f' (brand "{shell.brand}")' if shell.brand else ""
                lines.append(f"  {shell.name}: {shell.container}{brand} — {shell.action}")
    else:
        lines.append("shells: 0")
    if skipped:
        lines.append(
            "skipped: " + ", ".join(f"{row['name']} ({row['reason']})" for row in skipped)
        )
    if not groups and not shells:
        lines.append("sessions-dedupe: nothing to do (no duplicates, no shells)")
    elif mode == "dry-run":
        lines.append("dry-run: no changes made; re-run with --apply to link duplicates")
    return "\n".join(lines) + "\n"


def run(args: argparse.Namespace) -> int:
    if args.dir:
        target = Path(os.path.expanduser(args.dir))
    else:
        target = default_sessions_dir(dict(os.environ))
    if not target.is_dir():
        sys.stderr.write(f"sessions-dedupe: refused: {target} is not a directory\n")
        return 3
    try:
        candidates, skipped, freshest, regular_files = scan(target)
    except OSError as error:
        sys.stderr.write(f"sessions-dedupe: error: cannot list {target}: {error}\n")
        return 1

    now = time.time()
    if not args.force and freshest is not None and now - freshest[1] < IN_FLIGHT_SECONDS:
        sys.stderr.write(
            f"sessions-dedupe: refused: {freshest[0]} changed "
            f"{max(0.0, now - freshest[1]):.0f}s ago; a session may be in flight "
            f"(use --force to override)\n"
        )
        return 3

    groups, errors = find_groups(candidates, args.min_size)
    shells = find_shells(candidates)
    had_pending = any(
        duplicate.action == "pending" for group in groups for duplicate in group.duplicates
    )

    mode = "apply" if args.apply else "dry-run"
    if args.apply and not errors:
        try:
            apply_groups(groups, target)
            if args.rename_shells:
                apply_shell_renames(shells, target)
        except OSError as error:
            errors.append(str(error))

    for group in groups:
        for duplicate in group.duplicates:
            if duplicate.action == "pending":
                duplicate.action = "would-link" if mode == "dry-run" else "unlinked"

    bytes_reclaimable = sum(group.bytes_reclaimable for group in groups)
    bytes_reclaimed = 0
    if mode == "apply" and not errors:
        bytes_reclaimed = sum(
            group.bytes_reclaimable
            for group in groups
            if any(duplicate.action == "linked" for duplicate in group.duplicates)
        )

    actionable = had_pending or bool(shells)

    symlinks = sum(1 for row in skipped if row["reason"] == "symlink")
    report: dict[str, object] = {
        "schema": REPORT_SCHEMA,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "dir": str(target),
        "mode": mode,
        "min_size": args.min_size,
        "reclaim_note": "byte accounting assumes no hardlinks outside this directory",
        "totals": {
            "regular_files": regular_files,
            "candidates": len(candidates),
            "symlinks_skipped": symlinks,
            "duplicate_groups": len(groups),
            "duplicate_names": sum(len(group.duplicates) for group in groups),
            "bytes_reclaimable": bytes_reclaimable,
            "bytes_reclaimed": bytes_reclaimed,
            "shells": len(shells),
        },
        "groups": [group_json(group) for group in groups],
        "shells": [shell_json(shell) for shell in shells],
        "skipped": skipped,
        "errors": errors,
    }
    sys.stdout.write(
        human_summary(
            target=target,
            mode=mode,
            regular_files=regular_files,
            candidates=len(candidates),
            symlinks=symlinks,
            groups=groups,
            shells=shells,
            skipped=skipped,
            bytes_reclaimable=bytes_reclaimable,
            bytes_reclaimed=bytes_reclaimed,
        )
    )
    if args.json is not None:
        payload = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
        try:
            Path(args.json).write_text(payload, encoding="utf-8")
        except OSError as error:
            sys.stderr.write(f"sessions-dedupe: error: cannot write {args.json}: {error}\n")
            return 1
    if errors:
        for line in errors:
            sys.stderr.write(f"sessions-dedupe: error: {line}\n")
        return 1
    return 0 if actionable else 2


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dir",
        default=None,
        help="sessions directory (default: $CODESCRIBE_DATA_DIR/sessions or "
        "~/.codescribe/sessions)",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="write changes; default is a dry-run that changes nothing",
    )
    parser.add_argument(
        "--rename-shells",
        action="store_true",
        help="with --apply: rename <id>.wav shells to their real container "
        "extension and print the mapping",
    )
    parser.add_argument(
        "--json",
        metavar="PATH",
        default=None,
        help="also write the machine-readable report to PATH (keep it outside "
        "the scanned directory)",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="override the 60-second in-flight refusal",
    )
    parser.add_argument(
        "--min-size",
        type=int,
        default=1,
        help="minimum file size in bytes for duplicate grouping (default: 1)",
    )
    args = parser.parse_args()
    if args.rename_shells and not args.apply:
        parser.error("--rename-shells requires --apply")
    if args.min_size < 0:
        parser.error("--min-size must be >= 0")
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
