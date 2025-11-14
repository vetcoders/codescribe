#!/usr/bin/env python3
"""Generate or compare a hashed manifest for the VistaScribe repository.

The manifest format captures SHA256 hashes and sizes for tracked files. It can
operate on the working tree or any git ref, and it supports focused path
filters (e.g., only `tests/` or `.github/workflows`).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime

try:  # Python 3.11+
    from datetime import UTC
except ImportError:  # pragma: no cover - older runtime
    UTC = UTC  # type: ignore[misc]
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass
class Entry:
    path: str
    sha256: str
    size: int


class GitError(RuntimeError):
    pass


def run_git(args: list[str]) -> str:
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO_ROOT), *args],
            check=True,
            capture_output=True,
            text=True,
        )
        return out.stdout
    except subprocess.CalledProcessError as exc:  # pragma: no cover - rare
        raise GitError(f"git {' '.join(args)} failed: {exc.stderr.strip()}".strip()) from exc


def list_tracked_files(ref: str | None) -> list[str]:
    if ref:
        output = run_git(["ls-tree", "-r", "--name-only", ref])
    else:
        output = run_git(["ls-files"])
    return [line.strip() for line in output.splitlines() if line.strip()]


def should_include(path: str, filters: list[str]) -> bool:
    if not filters:
        return True
    return any(path == f or path.startswith(f.rstrip("/") + "/") for f in filters)


def hash_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_blob(path: str, ref: str | None) -> bytes:
    if ref:
        try:
            result = subprocess.run(
                ["git", "-C", str(REPO_ROOT), "show", f"{ref}:{path}"],
                check=True,
                capture_output=True,
            )
        except subprocess.CalledProcessError as exc:  # pragma: no cover - rare
            err = (exc.stderr or b"").decode("utf-8", "replace").strip()
            raise GitError(f"git show {ref}:{path} failed: {err}") from exc
        return result.stdout
    return (REPO_ROOT / path).read_bytes()


def build_manifest(ref: str | None, paths: list[str]) -> dict[str, Entry]:
    entries: dict[str, Entry] = {}
    for path in list_tracked_files(ref):
        if not should_include(path, paths):
            continue
        try:
            data = read_blob(path, ref)
        except FileNotFoundError:
            continue
        except GitError:
            continue
        entries[path] = Entry(path=path, sha256=hash_bytes(data), size=len(data))
    return entries


def manifest_dict(ref: str | None, paths: list[str]) -> dict[str, object]:
    entries = build_manifest(ref, paths)
    return {
        "generated_at": datetime.now(UTC).isoformat(),
        "ref": ref or "WORKTREE",
        "paths": paths or ["<all>"],
        "entries": {p: {"sha256": e.sha256, "size": e.size} for p, e in entries.items()},
    }


def write_manifest(output: Path, data: dict[str, object]) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def diff_manifests(
    ref_a: str | None,
    ref_b: str | None,
    paths: list[str],
) -> dict[str, list[dict[str, object]]]:
    a = build_manifest(ref_a, paths)
    b = build_manifest(ref_b, paths)

    added = [
        {"path": path, "sha256": entry.sha256, "size": entry.size}
        for path, entry in sorted(b.items())
        if path not in a
    ]
    removed = [
        {"path": path, "sha256": entry.sha256, "size": entry.size}
        for path, entry in sorted(a.items())
        if path not in b
    ]
    changed = [
        {
            "path": path,
            "from": a[path].sha256,
            "to": b[path].sha256,
            "size_from": a[path].size,
            "size_to": b[path].size,
        }
        for path in sorted(set(a) & set(b))
        if a[path].sha256 != b[path].sha256
    ]
    return {"added": added, "removed": removed, "changed": changed}


def cmd_generate(args: argparse.Namespace) -> int:
    data = manifest_dict(args.ref, args.paths)
    output = REPO_ROOT / args.output
    write_manifest(output, data)
    print(f"Manifest written to {output}")
    return 0


def cmd_diff(args: argparse.Namespace) -> int:
    if args.ref_a or args.ref_b:
        ref_a = args.ref_a
        ref_b = args.ref_b
    elif args.ref:
        ref_a = None
        ref_b = args.ref
    else:
        ref_a = None
        ref_b = "HEAD"
    if args.working_tree_a:
        ref_a = None
    if args.working_tree_b:
        ref_b = None
    result = diff_manifests(ref_a, ref_b, args.paths)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    else:
        for key in ("added", "removed", "changed"):
            items = result[key]
            if not items:
                continue
            print(f"{key.upper()} ({len(items)}):")
            for item in items:
                if key == "changed":
                    print(f"  {item['path']}: {item['from'][:12]} → {item['to'][:12]}")
                else:
                    print(f"  {item['path']}: {item['sha256'][:12]}")
    if args.fail_on_diff and any(result.values()):
        return 1
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    gen = sub.add_parser("generate", help="Write manifest.json for the current tree or ref.")
    gen.add_argument("--ref", help="Git ref to read (default: working tree)")
    gen.add_argument("--paths", nargs="*", default=[], help="Optional path prefixes to include")
    gen.add_argument(
        "--output",
        default="manifest.json",
        help="Relative path for the manifest (default: manifest.json)",
    )
    gen.set_defaults(func=cmd_generate)

    diff = sub.add_parser("diff", help="Compare manifests between refs or the working tree.")
    diff.add_argument("--ref", help="Compare working tree against this ref", default=None)
    diff.add_argument("--ref-a", help="Explicit left ref")
    diff.add_argument("--ref-b", help="Explicit right ref")
    diff.add_argument("--paths", nargs="*", default=[], help="Optional path prefixes to include")
    diff.add_argument("--json", action="store_true", help="Emit JSON diff")
    diff.add_argument(
        "--fail-on-diff", action="store_true", help="Exit with non-zero status when diff exists"
    )
    diff.add_argument(
        "--working-tree-a",
        action="store_true",
        help="Force left input to use the working tree",
    )
    diff.add_argument(
        "--working-tree-b",
        action="store_true",
        help="Force right input to use the working tree",
    )
    diff.set_defaults(func=cmd_diff)
    return parser


def main(argv: Iterable[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
