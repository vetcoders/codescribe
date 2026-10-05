"""Refresh only the canonical publisher digest after nested code signing.

Every other payload byte must still match its pre-signing manifest. The caller
reseals the outer app immediately after this operation and verifies its seal.
"""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import sys


def refresh(root: Path):
    manifest_path = root / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("schema") != "codescribe.agent-bridge.bundle.v1":
        raise ValueError("invalid agent bridge manifest")
    entries = manifest["files"]
    seen = set()
    for entry in entries:
        relative = PurePosixPath(entry["path"])
        if relative.is_absolute() or ".." in relative.parts or str(relative) in seen:
            raise ValueError("invalid or duplicate payload path")
        seen.add(str(relative))
        path = root / relative
        if (not path.is_file() or path.is_symlink() or not path.resolve().is_relative_to(root)
                or any(parent.is_symlink() for parent in path.parents if parent != root and root in parent.parents)):
            raise ValueError("payload must contain ordinary files")
        size = path.stat().st_size
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for block in iter(lambda: handle.read(1 << 20), b""):
                digest.update(block)
        checksum = digest.hexdigest()
        if str(relative) == "bin/codescribe":
            if not 0 < size <= 256 << 20 or not os.access(path, os.X_OK):
                raise ValueError("invalid signed publisher")
            entry["sha256"] = checksum
            entry["bytes"] = size
        elif entry["sha256"] != checksum or entry["bytes"] != size:
            raise ValueError("signing changed an unrelated agent bridge payload")
    if "bin/codescribe" not in seen:
        raise ValueError("signed app must contain its canonical publisher")
    temporary = manifest_path.with_suffix(".signing.json")
    temporary.write_text(json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, manifest_path)


if __name__ == "__main__":
    refresh(Path(sys.argv[1]).resolve())
