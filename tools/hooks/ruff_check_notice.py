#!/usr/bin/env python3
"""Run `ruff check --fix` and emit a friendly note when fixes were applied.

Exits non-zero only when Ruff reports remaining issues (same behavior as
`ruff check --fix`).
"""

from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path

MESSAGE = (
    "We've found lint issues, but they were automatically repaired by the Ruff "
    "pre-commit checks. Anyway it is advised to recheck your files for potential issues. "
    "If you are sure you feel comfortable committing the files once again, it should easily pass."
)


def file_hash(path: Path) -> str | None:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except FileNotFoundError:
        return None


def run_ruff(files: list[Path]) -> int:
    cmd = [sys.executable, "-m", "ruff", "check", "--fix", *[str(p) for p in files]]
    return subprocess.run(cmd).returncode


def main(argv: list[str]) -> int:
    files = [Path(arg) for arg in argv]
    before: dict[Path, str | None] = {path: file_hash(path) for path in files}
    code = run_ruff(files)
    if code != 0:
        return code
    changed = [path for path in files if before[path] != file_hash(path)]
    if changed:
        print(MESSAGE)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
