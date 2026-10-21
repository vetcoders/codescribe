#!/bin/bash
# target-source-guard.sh - Rebuild workspace crates another checkout compiled
#
# Usage:
#   scripts/target-source-guard.sh CARGO_TARGET_DIR
#
# Every worktree of this repository builds into the main checkout's target
# directory, and the workspace crates get the same artifact names in each of
# them: Cargo hands rustc paths relative to the workspace root and judges a
# path crate fresh when its source files are older than its last build. After a
# build in another worktree, this checkout can link or run that worktree's code
# without compiling a line of its own (2026-10-21: a root test run executed a
# worktree's ledger and reported its results as root's).
#
# rustc still records the absolute directory it compiled in: in each crate's
# metadata (.rmeta, or the .rlib when the .rmeta is empty) and in the debug
# paths of every binary, test harness and dylib it links. The guard looks at
# each workspace target's artifacts written since its last look from this
# checkout. When one was compiled in another checkout and is newer than this
# checkout's source root for that target, Cargo would take it as fresh here,
# so the guard touches the source root of every workspace target in this
# checkout and Cargo rebuilds the workspace crates from these sources;
# dependencies stay shared. Nothing is deleted.
#
# Builds started at the same moment from two checkouts can still interleave;
# serialize them through the build announcement on channel 0.

set -euo pipefail

TARGET_DIR="${1:?usage: target-source-guard.sh CARGO_TARGET_DIR}"
CHECKOUT="$(git rev-parse --show-toplevel)"

cargo metadata --no-deps --format-version 1 --manifest-path "$CHECKOUT/Cargo.toml" |
    python3 -I -c '
import glob, hashlib, json, mmap, os, sys, time

checkout, target_dir = sys.argv[1], sys.argv[2]
meta = json.load(sys.stdin)
targets = [t for p in meta["packages"] for t in p["targets"]]

seen_dir = os.path.join(target_dir, ".target-source-guard")
seen = os.path.join(seen_dir, hashlib.sha1(checkout.encode()).hexdigest()[:16])
last_look = os.stat(seen).st_mtime if os.path.exists(seen) else 0.0
started = time.time()
ours_prefix = checkout.encode()
PRINTABLE = range(0x21, 0x7F)
# Linked binaries follow a source path with "/@/" debug-map markers, so only a
# character that would continue the file name ends a match.
NAME_CONTINUES = frozenset(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-")


def recent_artifacts():
    """Non-empty files in every deps/ and examples/ written since the last look."""
    for parent in glob.glob(os.path.join(target_dir, "*")) + glob.glob(os.path.join(target_dir, "*", "*")):
        for directory in ("deps", "examples"):
            folder = os.path.join(parent, directory)
            if not os.path.isdir(folder):
                continue
            with os.scandir(folder) as entries:
                for entry in entries:
                    if not entry.is_file() or entry.name.endswith((".d", ".o")):
                        continue
                    stat = entry.stat()
                    if stat.st_mtime > last_look and stat.st_size:
                        yield entry.name, entry.path, stat.st_mtime


def belongs_to(name, crate):
    """Metadata, library, dylib, or a binary/test harness Cargo built for `crate`."""
    return (
        (name.startswith(f"lib{crate}-") and name.endswith((".rmeta", ".rlib")))
        or (name.startswith(f"lib{crate}") and name.endswith(".dylib"))
        or (name.startswith(f"{crate}-") and "." not in name)
    )


def compiled_elsewhere(data, suffix):
    """The other checkout a file was compiled in, or None when it is ours."""
    end = len(data)
    other = None
    while True:
        at = data.rfind(suffix, 0, end)
        if at < 0:
            return other
        end = at
        after = at + len(suffix)
        if after < len(data) and data[after] in NAME_CONTINUES:
            continue
        if data[max(0, at - len(ours_prefix)):at] == ours_prefix:
            return None
        start = at
        while start > 0 and data[start - 1] in PRINTABLE:
            start -= 1
        found = bytes(data[start:at])
        anchor = found.rfind(b"/Users/") if b"/Users/" in found else found.rfind(b"/Volumes/")
        other = (found[anchor:] if anchor >= 0 else found).decode(errors="replace")


recent = list(recent_artifacts())
foreign = set()
for target in targets:
    crate = target["name"].replace("-", "_")
    suffix = b"/" + os.path.relpath(target["src_path"], checkout).encode()
    root_mtime = os.stat(target["src_path"]).st_mtime
    for name, path, mtime in recent:
        if mtime <= root_mtime or not belongs_to(name, crate):
            continue
        try:
            with open(path, "rb") as handle, mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as data:
                other = compiled_elsewhere(data, suffix)
        except (OSError, ValueError) as error:
            print(f"target-source-guard: cannot read {path}: {error}", file=sys.stderr)
            continue
        if other:
            foreign.add(other)

if foreign:
    for target in targets:
        if os.path.exists(target["src_path"]):
            os.utime(target["src_path"])
    print(
        "target-source-guard: workspace crates in the shared target were compiled in "
        + ", ".join(sorted(foreign))
        + f"; they rebuild from {checkout}",
        file=sys.stderr,
    )

os.makedirs(seen_dir, exist_ok=True)
with open(seen, "a"):
    pass
os.utime(seen, (started, started))
' "$CHECKOUT" "$TARGET_DIR"
